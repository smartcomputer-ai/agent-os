from __future__ import annotations
import logging
import asyncio
import inspect
from typing import Awaitable, Callable
from grit import *
from wit import *
from .resolvers import Resolver
from .query_executor import QueryExecutor

MailboxUpdate = tuple[ActorId, ActorId, MessageId] # sender_id, recipient_id, message_id
MailboxUpdateCallback = Callable[[set[MailboxUpdate]], Awaitable[None]]

class GenesisMessageNotReadyError(Exception):
    pass

class ExecutionContext:
    store:ObjectStore
    references:References
    resolver:Resolver
    query_executor:QueryExecutor
    agent_name:str
    agent_id:ActorId
    async_semaphore:asyncio.Semaphore|None
    sync_semaphore:asyncio.Semaphore|None

    def __init__(self):
        self.store = None
        self.references = None
        self.resolver = None
        self.query_executor = None
        self.agent_name = None
        self.agent_id = None
        self.async_semaphore = None
        self.sync_semaphore = None

    @classmethod
    def from_store(cls, store:ObjectStore, references:References, resolver:Resolver, agent_id:ActorId) -> ExecutionContext:
        ctx = cls()
        ctx.store = store
        ctx.references = references
        ctx.resolver = resolver
        ctx.agent_id = agent_id
        ctx.query_executor = None
        ctx.agent_name = None
        return ctx
    
class ActorExecutor:
    '''Runs an actor's steps whenever new inbox messages arrive. Use factory methods to create an instance.'''
    ctx:ExecutionContext
    actor_id:ActorId
    
    _step_lock:asyncio.Lock
    _step_sleep_event:asyncio.Event
    _step_cancel_event:asyncio.Event

    _last_step_id:StepId|None
    _last_step_inbox:Mailbox
    _last_step_outbox:Mailbox
    _current_inbox:Mailbox

    _logger:logging.Logger

    def __init__(
            self,
            ctx:ExecutionContext,
            actor_id:ActorId, 
            last_step_id:StepId|None, 
            last_step_inbox:Mailbox, 
            last_step_outbox:Mailbox):
        self._logger = logging.getLogger(f"{type(self).__name__}({actor_id.hex()})")
        if not isinstance(ctx, ExecutionContext):
            raise TypeError(f"ctx must be an ExecutionContext, got {type(ctx)}")
        self.ctx = ctx
        if not isinstance(actor_id, ActorId):
            raise TypeError(f"actor_id must be an ActorId, got {type(actor_id)}")
        self.actor_id = actor_id

        if(last_step_id is not None and not isinstance(last_step_id, StepId)):
            raise TypeError(f"last_step_id must be a StepId, got {type(last_step_id)}")
        self._last_step_id = last_step_id
        self._last_step_inbox = last_step_inbox
        self._last_step_outbox = last_step_outbox
        self._current_inbox = last_step_inbox.copy()
        #create the locks and events here so they are bound the the current event loop
        self._step_lock = asyncio.Lock()
        self._step_sleep_event = asyncio.Event()
        self._step_cancel_event = asyncio.Event()

        
    @classmethod
    def from_genesis(cls, ctx:ExecutionContext, actor_id:ActorId) -> ActorExecutor:
        return cls(ctx, actor_id, None, Mailbox(), Mailbox())

    @classmethod
    async def from_last_step(cls, ctx:ExecutionContext, actor_id:ActorId, last_step_id:StepId) -> 'ActorExecutor':
        last_step = await ctx.store.load(last_step_id)
        if(last_step is None):
            raise Exception(f"Could not load last step {last_step_id} for actor {actor_id.hex()}")
        if(last_step.inbox is not None):
            last_step_inbox = await ctx.store.load(last_step.inbox)
        else:
            last_step_inbox = Mailbox()
        if(last_step.outbox is not None):
            last_step_outbox = await ctx.store.load(last_step.outbox)
        else:
            last_step_outbox = Mailbox()
        return cls(ctx, actor_id, last_step_id, last_step_inbox, last_step_outbox)

    @property
    def actor_id_str(self) -> str:
        return self.actor_id.hex()

    async def get_current_inbox(self) -> Mailbox:
        async with self._step_lock:
            return self._current_inbox.copy()
        
    async def get_current_outbox(self) -> Mailbox:
        async with self._step_lock:
            return self._last_step_outbox.copy() #the current outbox = the last step's outbox

    async def update_current_inbox(self, new_messages:list[MailboxUpdate]):
        # An actor_id can appear multiple times in the new_messages list,
        # since it might have sent more than one message since the last step.
        # The runtime ensures that the messages arrive in order.
        # To contruct the current inbox (needed to run the current step), use the last messages received from each actor.
        # It is always possible to retreive previous messages since each message contains a reference to the previous message.
        # For example, if agent X sends A -> B -> C, they will always arrive in that order, not C -> A -> B, etc.
        # However, if a sender sends multiple messages, an update event might be skipped, but it will still be ordered, it might be A -> C
        async with self._step_lock:
            for sender_id, _recipient_id, message_id in new_messages:
                self._current_inbox[sender_id] = message_id
        self._step_sleep_event.set()

    def stop(self):
        self._step_cancel_event.set()
        #also wake from sleep
        self._step_sleep_event.set()

    async def start(self, outbox_callback:MailboxUpdateCallback):
        await asyncio.sleep(0) #yield to allow other tasks to run
        while not self._step_cancel_event.is_set():
            async with self._step_lock:
                new_inbox = self._current_inbox.copy()
            #__last_step_inbox is only accessed in this loop, so it is safe to check it outside the lock
            should_run_step = await self._should_run_step()
            if self._last_step_inbox == new_inbox and not should_run_step:
                #wait for an update to the __current_inbox (see set() call above)
                await self._step_sleep_event.wait()
            else:
                #start next step
                # before running the function, snapshot the outbox (to detect changes)
                exec_last_outbox = await self._load_step_outbox(self._last_step_id)
                #retrieve the wit transition function and start a task to run it
                try:
                    wit_execution = await self._create_wit_execution(new_inbox)
                except GenesisMessageNotReadyError:
                    continue
                #starts the wit task but does not return it, has to be awaited separately
                await wit_execution.run(self.ctx)
                #todo: check if this can be interrupted and do not await it if that is the case
                #throw any exceptions
                #todo: handle exceptions gracefully: question: how to crash the actor?
                try:
                    await wit_execution.run_task
                    new_step_id = wit_execution.run_task.result()
                    if not isinstance(new_step_id, StepId):
                        raise TypeError(f"Expected a StepId, got {type(new_step_id)}")
                except Exception as e:
                    self._logger.exception(f"Exception in actor '{self.actor_id_str}' when executing wit function: {e}")
                    raise e
                #set the new step id as the HEAD for this actor
                await self.ctx.references.set(ref_step_head(self.actor_id), new_step_id)
                exec_new_inbox = await self._load_step_inbox(new_step_id)
                exec_new_outbox = await self._load_step_outbox(new_step_id)
                #move step forward, by making the new step the last step
                async with self._step_lock:
                    self._last_step_id = new_step_id
                    self._last_step_inbox = exec_new_inbox
                    self._last_step_outbox = exec_new_outbox
                #compare the last outbox to the new outbox and signal the runtime of any changes
                # the updates are a set because the specific order of the outbox messages is not relevant
                # it's just the set of the last known new message ids to the recipients
                new_outbox_messages:set[MailboxUpdate] = set()
                for recipient_id, message_id in exec_new_outbox.items():
                    if recipient_id not in exec_last_outbox or exec_last_outbox[recipient_id] != message_id:
                        new_outbox_messages.add((self.actor_id, recipient_id, message_id))
                if len(new_outbox_messages) > 0:
                    #self._logger.debug(f"{type(self)}: callback")
                    await outbox_callback(new_outbox_messages)
            #clear the event so it can wait again later
            self._step_sleep_event.clear()

    async def _should_run_step(self)->bool:
        '''Allows for a sub-class to override the logic of when to run the step function.'''
        return False    

    async def _create_wit_execution(self, new_inbox:Mailbox) -> _WitExecution:
        try:
            last_step_inbox = await self._load_step_inbox(self._last_step_id)
            return await _WitExecution.from_new_inbox(self.ctx, self.actor_id, self._last_step_id, last_step_inbox, new_inbox)
        except GenesisMessageNotReadyError as e:
            raise e
        except Exception as e:
            self._logger.exception("error creating wit execution.")
            raise e
        
    async def _load_step_inbox(self, step_id:StepId|None) -> Mailbox:
        if(step_id is None):
            return {}
        else:
            step:Step = await self.ctx.store.load(step_id)
            if(step.inbox is None):
                return {}
            return await self.ctx.store.load(step.inbox)
    
    async def _load_step_outbox(self, step_id:StepId|None) -> Mailbox:
        if(step_id is None):
            return {}
        else:
            step:Step = await self.ctx.store.load(step_id)
            if(step.outbox is None):
                return {}
            return await self.ctx.store.load(step.outbox)
    

class _WitExecution:
    #set in from_new_inbox
    actor_id:ActorId
    is_genesis:bool
    is_update:bool
    last_step_id:StepId|None
    new_inbox:Mailbox
    new_inbox_messages: list[InboxMessage]
    executing_core_id:TreeId|None
    cancel_event:asyncio.Event
    can_cancel:bool

    #set in _resolve_func_from_core
    is_async:bool
    func:Callable

    #set in run
    is_running:bool
    run_task:asyncio.Task[StepId]

    def __init__(self, actor_id:ActorId, last_step_id:StepId, new_inbox:Mailbox):
        #set all
        self.actor_id = actor_id
        self.last_step_id = last_step_id
        self.new_inbox = new_inbox

        self.is_genesis = False
        self.is_update = False
        self.can_cancel = False

        self.new_inbox_messages = []
        self.executing_core_id = None
        self.cancel_event = asyncio.Event()
        self.is_async = None
        self.func = None
        self.is_running = False
        self.run_task = None

    @classmethod
    def from_fixed_function(cls, ctx:ExecutionContext, actor_id:ActorId, last_step_id:StepId, new_inbox:Mailbox, func:Callable):
        execution = cls(actor_id, last_step_id, new_inbox)
        execution.func = func
        execution.is_async = asyncio.iscoroutinefunction(func)
        return execution

    @classmethod
    async def from_new_inbox(cls, ctx:ExecutionContext, actor_id:ActorId, last_step_id:StepId, last_step_inbox:Mailbox, new_inbox:Mailbox):
        execution = cls(actor_id, last_step_id, new_inbox)
        tmp_inbox = Inbox(ctx.store, last_step_inbox, new_inbox)
        execution.new_inbox_messages = await tmp_inbox.read_new()

        # If last_step_id is None, then this is the genesis step
        if last_step_id is None:
            # Serch new_inbox_messages for a message with the same content id as this actor id.
            # The create message also must be the first message sent from the sender actor, so it has no previous message.
            genesis_msg = next((msg for msg in execution.new_inbox_messages if msg.content_id == actor_id and msg.previous_id is None), None)
            if genesis_msg is None:
                # the create message might simply have not arrived yet
                print(f"No genesis message found in actor {actor_id.hex()}, will wait and try again.")
                await asyncio.sleep(0.05)
                raise GenesisMessageNotReadyError("No genesis message found in the new_messages list.")
            execution.executing_core_id = genesis_msg.content_id
            execution.is_genesis = True
            # Make the new_inbox only contain the genesis message
            tmp_inbox = Inbox(ctx.store, last_step_inbox, new_inbox)
            tmp_inbox.set_read_manually(genesis_msg.sender_id, genesis_msg.message_id)
            execution.new_inbox = tmp_inbox.get_current()
        # Otherwise, the actor already exists, and we just need to look for update messages
        else:
            # See if there is an update message in the current message list, then use that core
            # But if not, simply use the core from the previous step
            update_msg = next((msg for msg in execution.new_inbox_messages if msg.mt == 'update'), None)
            if update_msg is not None:
                execution.executing_core_id = update_msg.content_id
                execution.is_update = True
                # Make the new_inbox only contain the update message, and other previously read messages
                tmp_inbox = Inbox(ctx.store, last_step_inbox, new_inbox)
                tmp_inbox.set_read_manually(update_msg.sender_id, update_msg.message_id)
                execution.new_inbox = tmp_inbox.get_current()
            else:
                last_step:Step = await ctx.store.load(last_step_id)
                execution.executing_core_id = last_step.core
                # Check if all the new messages are signals
                # if so, the executor can cancel the execution of this wit
                execution.can_cancel = all(msg.is_signal for msg in execution.new_inbox_messages)
        
        # Now load the function from the executing_core_id (whose origin can differ depending if it is a genesis, update, or normal message)
        if execution.executing_core_id is None:
            raise Exception("Cannot resolve wit function from core because executing_core_id is None.")
        if not execution.is_update :
            execution.func = await ctx.resolver.resolve(execution.executing_core_id, 'wit', True)
        else:
            # Is an update
            execution.func = await ctx.resolver.resolve(execution.executing_core_id, 'wit_update', False)
            if(execution.func is None):
                # Use the default update function, which merges the cores
                update_wit = Wit(fail_on_unhandled_message=True, generate_wut_query=False)
                update_wit.run_wit(default_update_wit)
                execution.func = update_wit
        if execution.func is None:
            raise Exception(f"Could not resolve wit function from core for actor '{actor_id.hex()}'.")
        
        # Check if the func is async or not
        unwrapped_func = inspect.unwrap(execution.func)
        # cannot simply check if only the function is async, because it might be a callable class which is defined as async
        # i.e. async def __call__(self, ...):
        # for example, this is the case with the 'Wit' API class, used for decorators
        execution.is_async = inspect.iscoroutinefunction(unwrapped_func) or (
            callable(unwrapped_func) and asyncio.iscoroutinefunction(unwrapped_func.__call__))
        return execution

    async def run(self, ctx:ExecutionContext) -> None:
        if self.func is None:
            raise Exception("Cannot run wit because func is None.")
        args = (self.last_step_id, self.new_inbox)
        kwargs = {
            'agent_id': ctx.agent_id,
            'actor_id': self.actor_id,
            'object_store': ctx.store,
            'store': ctx.store,
            'cancel_event': self.cancel_event,
        }
        task_name = f'wit_function_{self.actor_id}'
        if(self.is_async):
            #if there is a semaphore that limits how many async wits can execute in parallel, use it
            if ctx.async_semaphore is not None:
                async with ctx.async_semaphore:
                    self.run_task = asyncio.create_task(self.func(*args, **kwargs), name=task_name)
            else:
                self.run_task = asyncio.create_task(self.func(*args, **kwargs), name=task_name)
        else:
            #if there is a semaphore that limits how many sync wits can execute in parallel, use it
            if ctx.sync_semaphore is not None:
                async with ctx.sync_semaphore:
                    self.run_task = asyncio.create_task(asyncio.to_thread(self.func, *args, **kwargs), name=task_name)
            else:
                self.run_task = asyncio.create_task(asyncio.to_thread(self.func, *args, **kwargs), name=task_name)
        self.is_running = True

    async def cancel(self, timeout:float|None=None) -> None:
        if not self.is_running:
            return
        self.cancel_event.set()
        #wait for one second for the task to finish
        try:
            await asyncio.wait_for(self.run_task, timeout=timeout)
        except asyncio.TimeoutError:
            #if the task did not finish in time, cancel it
            self.run_task.cancel()
        finally:
            #wait for the task to finish
            await self.run_task
            #TODO: capture the cancel exception
        self.is_running = False

async def default_update_wit(inbox:Inbox, outbox:Outbox, core:Core):
    #expect an update message
    msg = await inbox.read_new()
    if len(msg) != 1:
        raise InvalidUpdateException("Expected exactly one update message in the inbox.")
    update_msg = msg[0]
    if(update_msg.mt != 'update'):
        raise InvalidUpdateException("Found one message in the inbox, but it was not an update, it must have the header 'mt'='update'.")
    new_core = await update_msg.get_content()
    if(not isinstance(new_core, TreeObject)):
        raise InvalidUpdateException("The update message did not contain a tree object.")
    #now, merge the new core into the current core
    #print(f"Updating core {core.get_as_object_id().hex()} with new core {update_msg.content_id.hex()} by merging them.")
    await core.merge(new_core)    


