
# An "In-Process Cluster" for testing
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.grit import *
from aos.wit import *
from .runtime import Runtime
from .resolvers import ExternalResolver

class InProcessCluster:
    def __init__(
            self, 
            wits:dict[str, Wit], 
            create_query_wit: bool = False,
            store:ObjectStore = None,
            refs:References = None,
            ) -> None:
        self.wits = wits
        self.create_query_wit = create_query_wit

        if store is not None:
            self.store = store
        else:
            self.store = MemoryObjectStore()
        if refs is not None:
            self.refs = refs
        else:
            self.refs = MemoryReferences()
        
        self.resolver = ExternalResolver(self.store)
        for wit_name, wit in self.wits.items():
            self.resolver.register(wit_name, wit)

        self.runtime = Runtime(store=self.store, references=self.refs, resolver=self.resolver)
        self._running_task:asyncio.Task = None

    async def __aenter__(self):
        await self.start()
        return self
    
    async def __aexit__(self, *args):
        await self.stop()

    async def start(self):
        self._running_task = asyncio.create_task(self.runtime.start())
        await asyncio.sleep(0.01)
        await self.create_actors()
        await asyncio.sleep(0.01)

    async def stop(self):
        self.runtime.stop()
        await asyncio.wait_for(self._running_task, timeout=1)

    async def create_genesis_message(self, wit_name:str) -> MailboxUpdate:
        '''Creates a genesis message and returns a MailboxUpdate'''
        gen_core:TreeObject = Core.from_external_wit_ref(wit_name, wit_name if self.create_query_wit else None)
        gen_message = await OutboxMessage.from_genesis(self.store, gen_core)
        gen_message_id = await gen_message.persist(self.store)
        return (self.runtime.agent_id, gen_message.recipient_id, gen_message_id)
    
    async def create_actors(self):
        for wit_name, wit in self.wits.items():
            gen_message = await self.create_genesis_message(wit_name)
            await self.runtime.inject_mailbox_update(gen_message)
            #create a named actor ref
            await self.refs.set(ref_actor_name(wit_name), gen_message[1])

    async def inject_content(self, actor_name:str, content:ValidMessageContent, is_signal:bool=False, mt:str|None=None):
        actor_id = await self.refs.get(ref_actor_name(actor_name))
        message = OutboxMessage.from_new(actor_id, content, is_signal=is_signal, mt=mt)
        await self.runtime.inject_message(message)

    async def get_actor(self, actor_name:str) -> ActorId:
        return await self.refs.get(ref_actor_name(actor_name))

    async def get_actor_step(self, actor_name:str) -> Step:
        actor_id = await self.refs.get(ref_actor_name(actor_name))
        step_id = await self.refs.get(ref_step_head(actor_id))
        return await self.store.load(step_id)
    
    async def get_actor_core(self, actor_name:str) -> Core:
        step = await self.get_actor_step(actor_name)
        return await Core.from_core_id(self.store, step.core)