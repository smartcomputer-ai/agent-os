from __future__ import annotations
from grit import *
from wit.errors import QueryError
from .resolvers import Resolver

class QueryExecutor:
    """Executes wit queries against an actor's head step."""
    loader:ObjectLoader
    references:References
    resolver:Resolver
    agent_id:ActorId
    
    def __init__(self, 
        loader:ObjectLoader, 
        references:References, 
        resolver:Resolver,
        agent_id:ActorId,):

        self.loader = loader
        self.references = references
        self.resolver = resolver
        self.agent_id = agent_id

    async def run(self, actor_id:ActorId, query_name:str, context:Blob|None) -> Tree | Blob:
        actor_id_str = actor_id.hex()
        current_step_id = await self.references.get(ref_step_head(actor_id))
        if(current_step_id is None):
            raise QueryError(f"Actor '{actor_id_str}' does not have a HEAD step, '{ref_step_head(actor_id)}'. "+
                             "Make sure its genesis step has completed.")
        current_step_id_str = current_step_id.hex()
        
        #load the current step
        current_step:Step = await self.loader.load(current_step_id)
        if(current_step is None):
            raise QueryError(f"Actor '{actor_id_str}' has a HEAD step '{current_step_id_str}' that does not exist.")
        if(not is_step(current_step)):
            raise QueryError(f"Actor '{actor_id_str}' has a HEAD step '{current_step_id_str}' that is not a step.")
        if(current_step.actor != actor_id):
            raise QueryError(f"Actor '{actor_id_str}' has a HEAD step '{current_step_id_str}' that does not belong to it. "+
                             "The actor inside the step doesnot match actor '{current_step.actor.hex()}'.")

        query_func = await self.resolver.resolve(current_step.core, 'wit_query', is_required=False)
        if(query_func is None):
            raise QueryError(f"Actor '{actor_id_str}' has no query function.")
        args = (current_step_id, query_name, context)
        kwargs ={
            'loader': self.loader,
            'object_loader': self.loader,
            'actor_id': actor_id,
            'agent_id': self.agent_id,
        } 
        try:
            result = await query_func(*args, **kwargs)
            return result
        except Exception as e:
            raise QueryError(f"Query '{query_name}' to '{actor_id_str}', with step '{current_step_id_str}', failed with an exception: {e}") from e


