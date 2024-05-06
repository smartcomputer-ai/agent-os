from __future__ import annotations
import logging
from aos.grit import *
from aos.runtime.core.external_storage_executor import ExternalStorageExecutor
from aos.wit.discovery import Discovery
from aos.wit.errors import QueryError
from aos.wit.external_storage import ExternalStorage
from aos.wit.query import Query
from .resolvers import Resolver

logger = logging.getLogger(__name__)

class QueryExecutor(Query):
    """Executes wit queries against an actor's head step."""
    loader:ObjectLoader
    references:References
    resolver:Resolver
    agent_id:ActorId
    
    def __init__(self, 
        loader:ObjectLoader, 
        references:References, 
        resolver:Resolver,
        agent_id:ActorId,
        discovery:Discovery|None=None,
        external_storage:ExternalStorageExecutor|None=None):

        self.loader = loader
        self.references = references
        self.resolver = resolver
        self.agent_id = agent_id
        self.discovery = discovery
        self.external_storage = external_storage

    async def run(self, actor_id:ActorId, query_name:str, context:Blob|None) -> Tree | Blob | None:
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
            'query': self
        }
        if(self.discovery is not None):
            kwargs['discovery'] = self.discovery
        if(self.external_storage is not None):
            kwargs['external_storage'] = self.external_storage.make_for_actor(actor_id.hex())
        try:
            result = await query_func(*args, **kwargs)
            return result
        except Exception as e:
            logger.error(f"Query '{query_name}' to '{actor_id_str}', with step '{current_step_id_str}', failed with an exception: {e}", exc_info=e)
            raise QueryError(f"Query '{query_name}' to '{actor_id_str}', with step '{current_step_id_str}', failed with an exception: {e}") from e


