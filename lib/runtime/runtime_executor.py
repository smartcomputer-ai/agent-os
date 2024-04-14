from __future__ import annotations
from grit import *
from grit import Mailbox
from wit import *
from .actor_executor import ExecutionContext, ActorExecutor, _WitExecution, MailboxUpdate

class RuntimeExecutor(ActorExecutor):
    """A special executor for the runtime agent, which communicates in the name of the runtime.
    
    Whenever a new message gets injected from the outside world, it still has to come from somewhere.
    And so external messages are injected into the runtime agent's outbox, and are then routed to the
    appropriate normal agent.

    It also supports to subscribe to messages sent by other actors to this runtime actor.
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
    async def from_agent_name(cls, ctx:ExecutionContext, agent_name:str) -> 'RuntimeExecutor':
        agent_id, last_step_id = await create_or_load_runtime_actor(ctx.store, ctx.references, agent_name)
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
        return RuntimeExecutor.ExternalMessageSubscription(self)

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
        def __init__(self, executor:RuntimeExecutor):
            self.executor = executor
            self.queue = asyncio.Queue()
        def __enter__(self):
            self.executor._external_message_subscriptions.add(self)
            return self.queue
        def __exit__(self, type, value, traceback):
            self.executor._external_message_subscriptions.remove(self)


async def create_or_load_runtime_actor(object_store:ObjectStore, references:References, agent_name:str) -> tuple[ActorId, StepId]:
    #check if the 'runtime/agent' reference exists
    agent_id = await references.get(ref_runtime_agent())
    if(agent_id is None):
        #if it doesn't exist, create it, and the first step for that agent (without actually running a wit)
        #the original agent core is simple, it just contains the name of the agent
        agent_genesis_core = Core(object_store, {}, None)
        agent_genesis_core.makeb("name").set_as_str(agent_name)
        #create a message from itself to itself: a good old bootstrap
        msg = await OutboxMessage.from_genesis(object_store, agent_genesis_core)
        msg_id = await msg.persist(object_store)
        agent_id = msg.recipient_id
        gen_inbox = {agent_id: msg_id} #the sender is the recipient here
        gen_inbox_id = await object_store.store(gen_inbox)
        #create the first step
        gen_step = Step(None, agent_id, gen_inbox_id, None, agent_id) #agent_id == genesis_core_id
        gen_step_id = await object_store.store(gen_step)
        #set initial references
        await references.set(ref_step_head(agent_id), gen_step_id)
        await references.set(ref_runtime_agent(), agent_id)
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
    
async def add_offline_message_to_runtime_outbox(
        object_store:ObjectStore, 
        references:References, 
        agent_name:str, 
        msg:OutboxMessage, 
        set_previous:bool=False,
        ) -> StepId:
    '''Dangerous! Do not use if the runtime is running!'''
    agent_id, last_step_id = await create_or_load_runtime_actor(object_store, references, agent_name)
    last_step = await object_store.load(last_step_id)
    if(last_step.outbox is None):
        last_step_outbox = {}
    else:
        last_step_outbox = await object_store.load(last_step.outbox)
    #set previous id if needed
    if(msg.recipient_id in last_step_outbox and set_previous and not msg.is_signal):
        msg.previous_id = last_step_outbox[msg.recipient_id]
    #new outbox
    new_outbox = last_step_outbox.copy()
    msg_id = await msg.persist(object_store)
    new_outbox[msg.recipient_id] = msg_id
    new_outbox_id = await object_store.store(new_outbox)
    #new step
    new_step = Step(last_step_id, agent_id, last_step.inbox, new_outbox_id, last_step.core)
    new_step_id = await object_store.store(new_step)
    await references.set(ref_step_head(agent_id), new_step_id)
    return new_step_id

async def remove_offline_message_from_runtime_outbox(
        object_store:ObjectStore, 
        references:References, 
        agent_name:str, 
        recipient_id:ActorId,
        ) -> StepId:
    '''Dangerous! Do not use if the runtime is running!'''
    #todo: dedupe the code with add_offline_message_to_agent_outbox
    agent_id, last_step_id = await create_or_load_runtime_actor(object_store, references, agent_name)
    last_step = await object_store.load(last_step_id)
    if(last_step.outbox is None):
        last_step_outbox = {}
    else:
        last_step_outbox = await object_store.load(last_step.outbox)
    #new outbox
    new_outbox = last_step_outbox.copy()
    if(recipient_id in new_outbox):
        del new_outbox[recipient_id]
    if(len(new_outbox) > 0):
        new_outbox_id = await object_store.store(new_outbox)
    else:
        new_outbox_id = None
    #new step
    new_step = Step(last_step_id, agent_id, last_step.inbox, new_outbox_id, last_step.core)
    new_step_id = await object_store.store(new_step)
    await references.set(ref_step_head(agent_id), new_step_id)
    return new_step_id

def agent_id_from_name(agent_name:str) -> ActorId:
    import grit.object_serialization as ser
    name_blob = Blob({'ct': 's'}, agent_name.encode('utf-8'))
    name_blob_id = ser.get_object_id(ser.blob_to_bytes(name_blob))
    core = {'name': name_blob_id}
    core_id = ser.get_object_id(ser.tree_to_bytes(core))
    return core_id #the agent id is the core id