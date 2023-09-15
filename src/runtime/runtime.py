from __future__ import annotations
import logging
import asyncio
from grit import *
from wit import *
from .resolvers import *
from .query_executor import QueryExecutor
from .actor_executor import ExecutionContext, ActorExecutor, MailboxUpdate
from .runtime_executor import RuntimeExecutor, agent_id_from_name

logger = logging.getLogger(__name__)

class Runtime:
    """The main runtime API to run an agent."""

    ctx:ExecutionContext

    __cancel_event:asyncio.Event
    __running_event:asyncio.Event
    __runtime_executor:RuntimeExecutor
    __executors:dict[ActorId, ActorExecutor]
    __executor_lock:asyncio.Lock
    # I *think* this cannot have a limit, otherwise it could cause deadlocks inside the run function of the executors
    # (the deadlock would actually be here in the callback, but technically it would be in the executor)
    __outbox_queue:asyncio.Queue

    def __init__(self, 
            store:ObjectStore, 
            references:References, 
            agent_name:str|None, 
            resolver:Resolver=None):
        if not isinstance(store, ObjectStore):
            raise TypeError('store must be an ObjectStore')
        if not isinstance(references, References):
            raise TypeError('references must be a References')
        if agent_name is not None and not isinstance(agent_name, str):
            raise TypeError('agent_name, if provided, must be a string.')
        
        self.ctx = ExecutionContext()
        self.ctx.store = store
        self.ctx.references = references
        if resolver is not None:
            self.ctx.resolver = resolver
        else:
            self.ctx.resolver = MetaResover.with_all(store)

        #if no agent_name was provided, it will be loaded later, in __init_agent_executor 
        if agent_name is not None:
            self.ctx.agent_name = agent_name
            self.ctx.agent_id = agent_id_from_name(agent_name)

        self.ctx.query_executor = QueryExecutor(self.ctx.store, self.ctx.references, self.ctx.resolver, self.ctx.agent_id)

        self.__cancel_event = asyncio.Event()
        self.__running_event = asyncio.Event()
        self.__executors = {}
        self.__runtime_executor = None
        self.__outbox_queue = asyncio.Queue()
        self.__executor_lock = asyncio.Lock()

    @property
    def agent_name(self) -> str|None:
        return self.ctx.agent_name
    @property
    def agent_id(self) -> ActorId:
        return self.ctx.agent_id

    #temporary
    @property
    def store(self) -> ObjectStore:
        return self.ctx.store
    @property
    def references(self) -> References:
        return self.ctx.references
    @property
    def resolver(self) -> Resolver:
        return self.ctx.resolver
    @property
    def query_executor(self) -> QueryExecutor:
        return self.ctx.query_executor

    #todo: add lock around __executors
    def get_actors(self) -> list[ActorId]:
        return list(self.__executors.keys())
    
    def actor_exists(self, actor_id:ActorId) -> bool:
        return actor_id in self.__executors
    
    async def get_actor_inbox(self, actor_id:ActorId) -> Mailbox:
        return await (self.__executors[actor_id].get_current_inbox())

    async def get_actor_outbox(self, actor_id:ActorId) -> Mailbox:
        return await (self.__executors[actor_id].get_current_outbox())

    async def __outbox_callback(self, outbox_updates:set[MailboxUpdate]):
        await self.__outbox_queue.put(outbox_updates)

    async def inject_mailbox_update(self, new_update:MailboxUpdate) -> MessageId:
        await self.__init_runtime_executor()
        '''Injects a "raw" mailbox update into the queue.
        Careful when rapidly sending more than one message to a specific actor:
        If no previous_id is set in the message, each message will be treated as a signals and only the last one might be processed.'''
        #print(f"inject   1 new messages to {new_update[1].hex()} AGENT OUTBOX  ({new_update[2].hex()})")
        await self.__runtime_executor.update_current_outbox([new_update])
        return new_update[2]

    async def inject_message(self, new_message:OutboxMessage) -> MessageId:
        await self.__init_runtime_executor()
        '''Injects a message into the queue. It will set the sender as the agent_id of this runtime.
        By default (unless the message is configured as a signal) this method will set the previous_id to the id that
        was sent to the recipient in the last inject_message call.'''

        #see if the previous_id should be set
        #otherwise, messages can get overriden because they are treated like a signal (signal = no previous_id)
        if(not new_message.is_signal):
            current_outbox = await self.__runtime_executor.get_current_outbox()
            if(new_message.recipient_id in current_outbox):
                new_message.previous_id = current_outbox[new_message.recipient_id]
        #use the agent_id as the sender_id
        new_update = await new_message.persist_to_mailbox_update(self.store, self.__runtime_executor.agent_id)
        return await self.inject_mailbox_update(new_update)
    
    async def __init_runtime_executor(self):
        async with self.__executor_lock:
            if(self.__runtime_executor is not None):
                return
            #TODO: support loading the agent even if no name was provided, from the refs and core
            self.__runtime_executor = await RuntimeExecutor.from_agent_name(self.ctx, self.agent_name)
            if(self.agent_id != self.__runtime_executor.agent_id):
                raise Exception(f"Agent name {self.agent_name} with id '{self.agent_id.hex()}' does not match the agent executor id "+
                                f"'{self.__runtime_executor.agent_id.hex()}'. Did you use the right name?")
    
    def wait_until_running(self) -> asyncio.Future:
        return self.__running_event.wait()

    def stop(self):
        self.__cancel_event.set()

    async def start(self):
        await self.__init_runtime_executor()
        refs = await self.references.get_all()
        actor_heads:dict[ActorId, StepId] = (
            {bytes.fromhex(ref.removeprefix('heads/')):step_id for ref,step_id in refs.items() if ref.startswith('heads/')}
            )

        async with self.__executor_lock:
            self.__executors:dict[ActorId, ActorExecutor] = {}
            for actor_id, step_id in actor_heads.items():
                if(actor_id != self.__runtime_executor.agent_id): # exclude the agent actor, which is managed separately
                    self.__executors[actor_id] = await ActorExecutor.from_last_step(self.ctx, actor_id, step_id)
            gather_executors = self.__executors.copy()
        
        # there might me messages that have not been processed in the last runtime exection, 
        # gather them now and add them to the top of the queue
        gather_executors[self.__runtime_executor.agent_id] = self.__runtime_executor
        await _gather_pending_messages_for_recipients(self.__outbox_queue, gather_executors)
        del gather_executors

        #start all executors
        executor_tasks:list[asyncio.Task] = []
        for executor in self.__executors.values():
            executor_tasks.append(asyncio.create_task(executor.start(self.__outbox_callback)))
        executor_tasks.append(asyncio.create_task(self.__runtime_executor.start(self.__outbox_callback)))

        #start processing the outbox queue for all the actors
        await asyncio.sleep(0) #yield to allow other tasks to run
        self.__running_event.set() #signal that the runtime is about to start
        while not self.__cancel_event.is_set():
            #print('waiting for outbox updates')
            outbox_updates:list[set[MailboxUpdate]] = []
            # await the outbox_updates queue with a timeout
            # this makes it possible to cancel the loop
            try:
                outbox_updates.append(await asyncio.wait_for(self.__outbox_queue.get(), 0.05) )
            except asyncio.TimeoutError:
                continue #test for cancel (in the while condition) and try again
            #gather all other pending updates that were added to the queue
            try:
                outbox_updates.append(self.__outbox_queue.get_nowait())
            except asyncio.QueueEmpty:
                pass
            #sort all new messages from the outbox updates, and group them by recipient
            actor_new_messages = _sort_new_messages_for_recipients(outbox_updates)
            #send the new messages to the actors
            for recipient_id, new_messages in actor_new_messages.items():
                #If the recipient "actor" is the runtime agent itself then send it to that executor.
                # Otherwise see if an actor executor exists, or create it.
                if(recipient_id == self.__runtime_executor.agent_id):
                    #print(f"sending  {len(new_messages)} new messages to {recipient_id.hex()} AGENT       ({new_messages[0][2].hex()})")
                    await self.__runtime_executor.update_current_inbox(new_messages)
                else:
                    if recipient_id in self.__executors:
                        #print(f"sending  {len(new_messages)} new messages to {recipient_id.hex()}         ({new_messages[0][2].hex()})")
                        await self.__executors[recipient_id].update_current_inbox(new_messages)
                    else:
                        #print(f"sending  {len(new_messages)} new messages to {recipient_id.hex()} GENESIS ({new_messages[0][2].hex()})")
                        #assume the actor does not exist and create a new genesis one
                        # TODO: better check in the referenes one more time
                        async with self.__executor_lock:
                            self.__executors[recipient_id] = ActorExecutor.from_genesis(self.ctx, recipient_id)
                        executor_tasks.append(asyncio.create_task(self.__executors[recipient_id].start(self.__outbox_callback)))
                        await self.__executors[recipient_id].update_current_inbox(new_messages)

        #cancel all executors
        for _, executor in self.__executors.items():
            executor.stop()
        self.__runtime_executor.stop()
        await asyncio.gather(*executor_tasks, return_exceptions=True)
        for executor_task in executor_tasks:
            ex = executor_task.exception()
            if(ex is not None):
                logger.error("exception in executor task")
                raise ex
    
    def subscribe_to_messages(self) -> RuntimeExecutor.ExternalMessageSubscription:
        return self.__runtime_executor.subscribe_to_messages()

def _sort_new_messages_for_recipients(outbox_updates:list[set[MailboxUpdate]]) -> dict[ActorId, list[MailboxUpdate]]:
    #gather all new messages from the outbox updates
    actor_new_messages = {}
    for outbox_update_set in outbox_updates:
        if(not isinstance(outbox_update_set, set)):
            raise TypeError(f"outbox_updates must contain all sets of tuples, not {type(outbox_update_set)}")
        for (sender_id, recipient_id, message_id) in outbox_update_set:
            actor_new_messages.setdefault(recipient_id, []).append((sender_id, recipient_id, message_id))
    return actor_new_messages

async def _gather_pending_messages_for_recipients(outbox_queue:asyncio.Queue, executors:dict[ActorId, ActorExecutor]):
    #cather all inboxes and outboxes to check if there are still any pending messages
    inboxes_outboxes:dict[ActorId, tuple[Mailbox, Mailbox]] = {}
    for actor_id, executor in executors.items():
        inboxes_outboxes[actor_id] = (await executor.get_current_inbox(), await executor.get_current_outbox())
    pending_messages = _find_pending_messages(inboxes_outboxes)
    for mailbox_update in pending_messages:
        await outbox_queue.put((mailbox_update))
    
def _find_pending_messages(inboxes_outboxes:dict[ActorId, tuple[Mailbox, Mailbox]]) -> list[MailboxUpdate]:
    inboxes = {actor_id: inbox for actor_id, (inbox, _) in inboxes_outboxes.items()}
    outboxes = {actor_id: outbox for actor_id, (_, outbox) in inboxes_outboxes.items()}
    pending_messages = []
    for sender_id, outbox in outboxes.items():
        #check each message in the outbox and see see if the corresponding agent's inbox matches it
        for recipient_id, message_id in outbox.items():
            # match each sender's outbox with the inbox of the recipient
            #  - the agent that owns the outbox is the sender, and its mailbox contains its recipients ids
            #  - conversely, the agent that owns the inbox is the recipient of the messages
            #  - so, if the recipient inbox does not contain the sender's outbox message_id, this message has not been processed by the recipient
            if(recipient_id not in inboxes or sender_id not in inboxes[recipient_id] or message_id != inboxes[recipient_id][sender_id]):
                pending_messages.append(set([(sender_id, recipient_id, message_id)]))
    #print("pending messages", len(pending_messages))
    return pending_messages



