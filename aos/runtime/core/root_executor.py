from __future__ import annotations
from aos.grit import *
from aos.grit import Mailbox
from aos.wit import *
from .actor_executor import ExecutionContext, ActorExecutor, _WitExecution, MailboxUpdate

class RootActorExecutor(ActorExecutor):
    """A special executor for 'root actor', which communicates in the name of the runtime.
    
    Whenever a new message gets injected from the outside world, it still has to come from somewhere.
    And so external messages are injected into the root actor's outbox, and are then routed to the
    appropriate normal actor.

    It also supports to subscribe to messages sent by other actors to this root actor.
    """
    _current_outbox:Mailbox
    _external_message_subscriptions:set[ExternalMessageSubscription]

    def __init__(self, 
        ctx:ExecutionContext,
        agent_id:ActorId, 
        last_step_id:StepId|None, 
        last_step_inbox:Mailbox, 
        last_step_outbox:Mailbox):
        super().__init__(
            ctx, agent_id, 
            last_step_id, last_step_inbox, last_step_outbox)
        self._current_outbox = last_step_outbox.copy()
        self._external_message_subscriptions = set()

    @classmethod
    async def from_agent_name(cls, ctx:ExecutionContext, agent_name:str) -> 'RootActorExecutor':
        agent_id, last_step_id = await create_or_load_root_actor(ctx.store, ctx.references, agent_name)
        return await cls.from_last_step(ctx, agent_id, last_step_id)
    
    @property
    def agent_id(self) -> ActorId:
        return self.actor_id
    
    @property
    def agent_id_str(self) -> str:
        return self.agent_id.hex()
    
    async def get_current_outbox(self) -> Mailbox:
        async with self._step_lock:
            return self._current_outbox.copy()

    async def update_current_outbox(self, new_messages:list[MailboxUpdate]):
        async with self._step_lock:
            for _sender_id, recipient_id, message_id in new_messages:
                self._current_outbox[recipient_id] = message_id
        self._step_sleep_event.set()
        
    async def _should_run_step(self) -> bool:
        '''In the base class, the step only runs if the inbox has changed, here it also needs to run if the outbox queue has changed.'''
        async with self._step_lock:
            return self._last_step_outbox != self._current_outbox

    async def _create_wit_execution(self, new_inbox:Mailbox) -> _WitExecution:
        return _WitExecution.from_fixed_function(
            self.ctx, 
            self.actor_id, 
            self._last_step_id, 
            new_inbox, 
            self.runtime_wit)
    
    async def runtime_wit(self, last_step_id:StepId, new_inbox:Mailbox, **kwargs) -> StepId:
        (inbox, outbox, core) = await load_step(self.ctx.store, self.actor_id, last_step_id, new_inbox)
        if(len(inbox.get_current()) > 0):
            #forward new messages to subscribers
            new_messages = await inbox.read_new()
            mailbox_updates:list[MailboxUpdate] = [(msg.sender_id, self.actor_id, msg.message_id,) for msg in new_messages]
            self._publish_to_external_subscribers(mailbox_updates)

        #persist the read inbox
        new_inbox_id = await inbox.persist(self.ctx.store)

        #inject pending outbox messages (injected by the runtime)
        async with self._step_lock:
            pending_outbox = self._current_outbox.copy()
        new_outbox = outbox.get_current()
        for recipient_id, message_id in pending_outbox.items():
            new_outbox[recipient_id] = message_id
        new_outbox_id = await self.ctx.store.store(new_outbox)

        #create the new step
        new_step = Step(last_step_id, self.actor_id, new_inbox_id, new_outbox_id, core.get_as_object_id()) #core has not changed
        new_step_id = await self.ctx.store.store(new_step)
        return new_step_id

    def stop(self):
        self._publish_stop_to_external_subscribers()
        super().stop()

    def subscribe_to_messages(self) -> ExternalMessageSubscription:
        return RootActorExecutor.ExternalMessageSubscription(self)

    def _publish_to_external_subscribers(self, new_messages:list[MailboxUpdate]):
        if(self._external_message_subscriptions is None or len(self._external_message_subscriptions) == 0):
            return
        for sub in self._external_message_subscriptions:
            for mailbox_update in new_messages:
                sub.queue.put_nowait(mailbox_update)

    def _publish_stop_to_external_subscribers(self):
        if(self._external_message_subscriptions is None or len(self._external_message_subscriptions) == 0):
            return
        for sub in self._external_message_subscriptions:
            sub.queue.put_nowait(None)

    class ExternalMessageSubscription:
        def __init__(self, executor:RootActorExecutor):
            self.executor = executor
            self.queue = asyncio.Queue()
        def __enter__(self):
            self.executor._external_message_subscriptions.add(self)
            return self.queue
        def __exit__(self, type, value, traceback):
            self.executor._external_message_subscriptions.remove(self)


async def create_or_load_root_actor(object_store:ObjectStore, references:References, agent_name:str) -> tuple[ActorId, StepId]:
    #TODO: see code in lmdb_store, it also creates a root actor. this code should be moved here, and lmdb_store should use it from here
    #      the change here needs to be that raw Grit objets are used to create the core, but that code already exists in lmdb_store
    
    #check if the 'runtime/agent' reference exists
    agent_id = await references.get(ref_root_actor())
    if(agent_id is None):
        #if it doesn't exist, create it, and the first step for that agent (without actually running a wit)
        #the original agent core is simple, it just contains the name of the agent
        last_id = None
        last_obj = None
        for obj in bootstrap_root_actor_objects(agent_name):
            object_id = await object_store.store(obj)
            last_id = object_id
            last_obj = obj
        gen_step_id = last_id # the last bootstrap object is the step, which is the genesis step
        agent_id = last_obj.actor
        if agent_id is None:
            raise ValueError("Agent id is None. Should not happen.")
        #set initial references
        await references.set(ref_step_head(agent_id), gen_step_id)
        await references.set(ref_root_actor(), agent_id)
        return agent_id, gen_step_id
    else:
        agent_genesis_core = await Core.from_core_id(object_store, agent_id)
        #check that the names match
        agent_genesis_core_name = (await agent_genesis_core.getb("name")).get_as_str()
        if(agent_genesis_core_name != agent_name):
            raise ValueError(f"Agent name mismatch: in agent genesis core: {agent_genesis_core_name}, but agent_name was {agent_name}")
        #load the last step
        last_step_id = await references.get(ref_step_head(agent_id))
        if(last_step_id is None):
            raise Exception(f"Agent {agent_name} has no reference: '{ref_step_head(agent_id)}'.")
        return agent_id, last_step_id


def bootstrap_root_actor_objects(agent_name:str, core_only:bool=False):
    """Iterates over all the grit objects from initial core, up to the genesis step, to 
    bootstrap the 'root actor' which represents the agent and the runtime."""
    import aos.grit.object_serialization as ser

    #initial core (which defines the agent id)
    name_blob = Blob({'ct': 's'}, agent_name.encode('utf-8'))
    yield name_blob
    name_blob_id = ser.get_object_id( ser.blob_to_bytes(name_blob))
    core = {'name': name_blob_id}
    yield core

    if(core_only):
        return
    
    core_id = ser.get_object_id(ser.tree_to_bytes(core))
    agent_id = core_id #the agent id is the core id
    
    #genesis message
    msg = Message(previous=None, headers={"mt": "genesis"}, content=core_id)
    yield msg
    msg_id = ser.get_object_id(ser.message_to_bytes(msg))

    #genesis step inbox (from agent id to itself, nice old bootstrap!)
    inbox = {agent_id: msg_id}
    yield inbox
    inbox_id = ser.get_object_id(ser.mailbox_to_bytes(inbox))

    #genesis step
    step = Step(previous=None, actor=agent_id, inbox=inbox_id, outbox=None, core=core_id)
    yield step


def bootstrap_root_actor_bytes(agent_name:str, core_only:bool=False):
    """Iterates over all the object ids and associated serialized data to bootstrap the root actor. 
    See: bootstrap_root_actor_objects"""
    import aos.grit.object_serialization as ser
    for obj in bootstrap_root_actor_objects(agent_name, core_only):
        data = ser.object_to_bytes(obj)
        yield ser.get_object_id(data), data


def agent_id_from_root_actor_name(agent_name:str) -> AgentId:
    """Generates the agent id from the agent name, by creating the root actor objects and extracting the agent id.
    See: bootstrap_root_actor_objects"""
    # how does this work? stop once the root actor core is created
    # the root actor core is the actor's id, and the root actor id is also the agent id
    agent_id, _ = list(bootstrap_root_actor_bytes(agent_name, core_only=True))[-1]
    return agent_id