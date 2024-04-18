
from __future__ import annotations
from abc import ABC
from dataclasses import dataclass
import os
import random
import asyncio
import grpc
from aos.grit import *
from enum import Enum
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc
from aos.runtime.store import agent_store_pb2, agent_store_pb2_grpc
from aos.runtime.apex import apex_api_pb2, apex_workers_pb2
from aos.runtime.store.store_client import StoreClient
from aos.runtime.store.agent_object_store import AgentObjectStore
from aos.runtime.store.agent_references import AgentReferences


import logging
logger = logging.getLogger(__name__)

# Steps:
# 0) get this node id
# 1) get all agents and their actors from grit
# 2) gather unprocessed messages (between actors)
# 3) wait for workers
# 4) assign actors to workers (compare actor's wit manifest to worker's capabilities)
# 5) send messages to workers 

# if new actor: make sure if it is a genesis or update message that the worker can handle the message)

class _EventWithResult(ABC):
    def __init__(self) -> None:
        self._result_event = asyncio.Event()
        self._result = None

    async def wait_for_result(self, timeout_seconds:float=90):
        """Wait for the callback to be called. If the callback is not called within the timeout, a asyncio.TimeoutError is raised."""
        await asyncio.wait_for(self._result_event.wait(), timeout_seconds)
        return self._result
    
    def set_result(self, result):
        self._result = result
        self._result_event.set()

class _EventWithCompletion(ABC):
    def __init__(self) -> None:
        self._completion_event = asyncio.Event()

    async def wait_for_completion(self, timeout_seconds:float=90):
        """Wait for the callback to be called. If the callback is not called within the timeout, a asyncio.TimeoutError is raised."""
        await asyncio.wait_for(self._completion_event.wait(), timeout_seconds)
    
    def set_completion(self):
        self._completion_event.set()

AgentId = ActorId #the agent is defined by the id of the root actor, so technically, it's an actor id too
AgendDID = str
WorkerId = str
MailboxUpdate = tuple[ActorId, ActorId, MessageId] # sender_id, recipient_id, message_id

APEX_STATUS_VAR_NAME = "apex.status"
APEX_STATUS_STARTED = "started"
APEX_STATUS_STOPPED = "stopped"

class ApexCoreLoop:

    @dataclass(frozen=True, slots=True)
    class _RegisterWorkerEvent:
        worker_id:str
        manifest:apex_workers_pb2.WorkerManifest
        ticket:str

    @dataclass(frozen=True, slots=True)
    class _WorkerConnectedEvent:
        worker_id:WorkerId
        ticket:str #used to verify that the handshake was correct
        to_worker_queue:asyncio.Queue[apex_workers_pb2.ApexToWorkerMessage]

    @dataclass(frozen=True, slots=True)
    class _WorkerDisconnectedEvent:
        worker_id:WorkerId

    @dataclass(frozen=True, slots=True)
    class _AssignActorsToWorkersEvent:
        new_workers:list[WorkerId]|None = None

    @dataclass(frozen=True, slots=True)
    class _RouteMessageEvent:
        worker_id:WorkerId|None #from which which worker, if None is generate by apex
        message:apex_workers_pb2.ActorMessage

        @property
        def is_to_root_actor(self):
            return self.message.recipient_id == self.message.agent_id

        @classmethod
        def from_mailbox_update(cls, agent_id:AgentId, mailbox_update:MailboxUpdate):
            return cls(worker_id=None, message=apex_workers_pb2.ActorMessage(
                agent_id=agent_id,
                sender_id=mailbox_update[0],
                recipient_id=mailbox_update[1],
                message_id=mailbox_update[2]))

    @dataclass(frozen=True, slots=True)
    class _RouteQueryEvent:
        worker_id:WorkerId
        query:apex_workers_pb2.ActorQuery

    @dataclass(frozen=True, slots=True)
    class _RouteQueryResultEvent:
        worker_id:WorkerId
        query_result:apex_workers_pb2.ActorQueryResult

    class _RunQueryEvent(_EventWithResult):
        query:apex_api_pb2.RunQueryRequest

        def __init__(self, query:apex_api_pb2.RunQueryRequest) -> None:
            super().__init__()
            self.query = query

        async def wait_for_result(self, timeout_seconds:float=90)-> apex_api_pb2.RunQueryResponse:
            return await super().wait_for_result(timeout_seconds)
    
    class _StartAgentEvent(_EventWithCompletion):
        agent_id:AgentId
        def __init__(self, agent_id:AgentId) -> None:
            super().__init__()
            self.agent_id = agent_id

    class _StopAgentEvent(_EventWithCompletion):
        agent_id:AgentId
        def __init__(self, agent_id:AgentId) -> None:
            super().__init__()
            self.agent_id = agent_id

    class _InjectMessageEvent(_EventWithResult):
        inject_request:apex_api_pb2.RunQueryRequest
        def __init__(self, inject_request:apex_api_pb2.InjectMessageRequest) -> None:
            super().__init__()
            self.inject_request = inject_request

        async def wait_for_result(self, timeout_seconds:float=15)-> apex_api_pb2.InjectMessageResponse:
            return await super().wait_for_result(timeout_seconds)
    
    @dataclass(slots=True)
    class _WorkerState:
        ticket:str
        manifest:apex_workers_pb2.WorkerManifest
        to_worker_queue:asyncio.Queue[apex_workers_pb2.ApexToWorkerMessage]|None = None
        asssigned_actors:set[ActorId] = set()

        @property
        def is_connected(self):
            return self.to_worker_queue is not None

    @dataclass(slots=True)
    class _ActorInfo:
        agent_id:AgentId
        manifest:str


    class _CoreLoopState:

        #TODO: move state modification in ecapsulated methods here (add agent, add actor, add message, assign actor unasign actor, etc)
        #      make it hard to mess up
        #      track if a copy should be made of the state at the end of the loop for external

        running_agents:dict[AgentId, AgendDID] = {}
        unassigned_actors:set[ActorId] = {} #are not assigned to a worker
        assigned_actors:dict[ActorId, WorkerId] = {} #are assigned to a worker

        actors:dict[ActorId, ApexCoreLoop._ActorInfo] = {}
        workers:dict[WorkerId, ApexCoreLoop._WorkerState] = {}

        unprocessed_messages:dict[ActorId, list[MailboxUpdate]] = {} #the recipient actor doesn't have a worker yet

        #unporcessed_queries
        #pending_queries (for callback and result routing)


    def __init__(
            self,
            store_address:str,
            node_id:str|None=None,
            ) -> None:
        
        if node_id is None:
            #create a random node id
            node_id = os.urandom(8).hex()

        self._node_id = node_id
        self._store_address = store_address
        self._cancel_event = asyncio.Event()
        self._running_event = asyncio.Event()
        self._event_queue = asyncio.Queue()

        
    async def start(self):
        logger.info("Starting apex core loop")
        loop_state = self._CoreLoopState()
        
        #try to connect to the store server
        store_client = await self._connect_to_store_loop()

        # gather started agents, their actors, and unprocessed messages
        self._gather_actors_for_started_agents(store_client, loop_state)

        #start processing of main loop
        await asyncio.sleep(0) #yield to allow other tasks to run
        self._running_event.set() #signal that apex is about to start
        while not self._cancel_event.is_set():
            event = None
            try:
                event = await asyncio.wait_for(self._event_queue.get(), 0.05)
            except asyncio.TimeoutError:
                continue #test for cancel (in the while condition) and try again
            
            if isinstance(event, self._RegisterWorkerEvent):
                self._handle_register_worker(event, loop_state)

            elif isinstance(event, self._WorkerConnectedEvent):
                self._handle_worker_connected(event, loop_state)

            elif isinstance(event, self._WorkerDisconnectedEvent):
                self._handle_worker_disconnected(event, loop_state)

            elif isinstance(event, self._AssignActorsToWorkersEvent):
                self._handle_assign_actors_to_workers(event, loop_state)
                
        #cleanup
        await store_client.close()


    async def stop(self):
        pass


    async def _connect_to_store_loop(self):
        logger.info("Connecting to store server...")
        tries = 0
        max_tries = 100
        while True:
            tries += 1
            try:
                store_client = StoreClient(self._store_address)
                await store_client.wait_for_async_channel_ready()
                logger.info("Connected to store server")
                return store_client
            except Exception as e:
                if tries >= max_tries:
                    logger.error(f"Max tries reached, giving up")
                    raise e
                else:
                    logger.warn(f"Was not able to connect to store server {self._store_address}, will try again: {e}")
                    await asyncio.sleep(5)

    
    async def _handle_register_worker(self, event:_RegisterWorkerEvent, loop_state:_CoreLoopState):
        #if there is an existing worker with the same id, disconnect it
        if event.worker_id in loop_state.workers:
            worker_state = loop_state.workers[event.worker_id]
            if worker_state.is_connected:
                logger.warn(f"RegisterWorkerEvent: Worker {event.worker_id} is already connected, disconnecting it.")
                worker_state.to_worker_queue.put_nowait(None)
        #add worker with new ticket
        loop_state.workers[event.worker_id] = self._WorkerState(
            ticket=event.ticket,
            manifest=event.manifest)
        logger.info(f"RegisterWorkerEvent: Worker {event.worker_id} registered.")


    async def _handle_worker_connected(self, event:_WorkerConnectedEvent, loop_state:_CoreLoopState):
        if event.worker_id not in loop_state.workers:
            logger.warn(f"WorkerConnectedEvent: Worker {event.worker_id} trying to connect, but it is not registered, NO-OP.")
            event.to_worker_queue.put_nowait(None)
        else:
            loop_state.workers[event.worker_id].to_worker_queue = event.to_worker_queue
            #assign actors to worker
            await self._event_queue.put(self._AssignActorsToWorkersEvent(new_workers=[event.worker_id]))
            logger.info(f"WorkerConnectedEvent: Worker {event.worker_id} connected.")


    async def _handle_worker_disconnected(self, event:_WorkerDisconnectedEvent, loop_state:_CoreLoopState):
        if event.worker_id not in loop_state.workers:
            logger.warn(f"WorkerDisconnectedEvent: Worker {event.worker_id} trying to disconnect, but it is not registered, NO-OP.")
        else:
            worker_state = loop_state.workers[event.worker_id]
            if worker_state.is_connected:
                #this might be important, because it allows the the server request processing task to terminate too
                worker_state.to_worker_queue.put_nowait(None)
            #move all assigned actors back to unassigned
            for actor_id in worker_state.asssigned_actors:
                loop_state.unassigned_actors.add(actor_id)
                del loop_state.assigned_actors[actor_id]
            
            del loop_state.workers[event.worker_id]
            await self._event_queue.put(self._AssignActorsToWorkersEvent())
            logger.info(f"WorkerDisconnectedEvent: Worker {event.worker_id} disconnected.")
        

    async def _handle_assign_actors_to_workers(self, event:_AssignActorsToWorkersEvent, loop_state:_CoreLoopState):
        #TODO: assign actors to worker, consider existing manifest
        # run algo that figures out which workers get which actors, 
        # rebalance, send relevant GIVE and YANK messages
        # in the first version, implement a greedy algo that assigns all actors to workers
        #
        # the problem is when we assign too greedly, then when workers come online on startup, the first one gets all actors

        # for testing, in the current version, just assign randomly
        to_assign = loop_state.unassigned_actors.copy()
        for actor_id in to_assign:
            #pick worker_id at random
            worker_ids = list(loop_state.workers.keys())
            worker_id = random.choice(worker_ids)
            loop_state.assigned_actors[actor_id] = worker_id
            loop_state.unassigned_actors.remove(actor_id)
            loop_state.workers[worker_id].asssigned_actors.add(actor_id)
            #tell worker that it has a new actor
            worker_state = loop_state.workers[worker_id]
            actor_info = loop_state.actors[actor_id]    
            await worker_state.to_worker_queue.put(apex_workers_pb2.ApexToWorkerMessage(
                type=apex_workers_pb2.ApexToWorkerMessage.GIVE_ACTOR,
                actor=apex_workers_pb2.Actor(
                    agent_id=actor_info.agent_id,
                    actor_id=actor_id,
                    grit_address=self._store_address
                )))
            logger.info(f"AssignActorsToWorkersEvent: Actor {actor_id} assigned to worker {worker_id}.")


    async def _handle_start_agent(self, event:_StartAgentEvent, loop_state:_CoreLoopState, store_client:StoreClient):
        #check if already running
        if event.agent_id in loop_state.running_agents:
            logger.warn(f"StartAgentEvent: Agent {event.agent_id.hex()} is already running, NO-OP.")
        else:
            agent_id = event.agent_id
            logger.info(f"StartAgentEvent: Starting agent {agent_id.hex()}.")
            #mark agent as started in the agent store
            await store_client.get_agent_store_stub_async().SetVar(
                agent_store_pb2.SetVarRequest(
                    agent_id=agent_id,
                    var_name=APEX_STATUS_VAR_NAME,
                    var_value=APEX_STATUS_STARTED))
            #gather actors and unpocessed messages
            references = AgentReferences(store_client, agent_id)
            object_store = AgentObjectStore(store_client, agent_id)
            await self._gather_actors_for_started_agent(object_store, references, agent_id, loop_state)
            
        event.set_completion()

    async def _handle_stop_agent(self, event:_StopAgentEvent, loop_state:_CoreLoopState, store_client:StoreClient):
        #check if already stopped
        if event.agent_id not in loop_state.running_agents:
            logger.warn(f"StopAgentEvent: Agent {event.agent_id.hex()} is not running, NO-OP.")
        else:
            agent_id = event.agent_id
            logger.info(f"StopAgentEvent: Stopping agent {agent_id.hex()}.")
            #mark agent as stopped in the agent store
            await store_client.get_agent_store_stub_async().SetVar(
                agent_store_pb2.SetVarRequest(
                    agent_id=agent_id,
                    var_name=APEX_STATUS_VAR_NAME,
                    var_value=APEX_STATUS_STOPPED))
            #remove actors and unprocessed messages
            
            #TODO
        
        event.set_completion()


    async def _gather_actors_for_started_agents(self, 
            store_client:StoreClient, 
            loop_state:_CoreLoopState):
        #get all agents that have a "started" status
        agents_response:agent_store_pb2.GetAgentsResponse = await (
            store_client
            .get_agent_store_stub_async()
            .GetAgents(agent_store_pb2.GetAgentsRequest(var_filters={APEX_STATUS_VAR_NAME, APEX_STATUS_STARTED})) 
        )
        loop_state.running_agents = {agent_id:did for did, agent_id in agents_response.agents.items()}
        logger.info(f"Found {len(loop_state.running_agents)} agents with status 'started'.")
        #get actors for all the agents
        for agent_id in loop_state.running_agents.keys():
            references = AgentReferences(store_client, agent_id)
            object_store = AgentObjectStore(store_client, agent_id)
            await self._gather_actors_for_started_agent(object_store, references, agent_id, loop_state)


    async def _gather_actors_for_started_agent(
            self,
            object_loader:ObjectLoader,
            references:References,
            agent_id:AgentId,
            loop_state:_CoreLoopState):

        #get all actors via refs
        refs = await references.get_all()
        actor_heads:dict[ActorId, StepId] = {bytes.fromhex(ref.removeprefix('heads/')):step_id for ref,step_id in refs.items() if ref.startswith('heads/')}

        named_actors = {ref.removeprefix('actors/'):actor_id for ref,actor_id in refs.items() if ref.startswith('actors/')}
        name_lookup = {actor_id:actor_name for actor_name,actor_id in named_actors.items()}
        prototype_actors = {ref.removeprefix('prototypes/'):actor_id for ref,actor_id in refs.items() if ref.startswith('prototypes/')}

        #set the actors
        # note: the root actor is handled differently, and is not included in the actors list
        actors_without_root = [actor_id for actor_id in actor_heads.keys() if actor_id != agent_id]
        for actor_id in actors_without_root:
            #todo: inspect the core of the actor to retrieve the manifest
            loop_state.actors[actor_id] = ApexCoreLoop._ActorInfo(agent_id, "TODO")

        #gather unprocessed messages
        # note: we DO want to include the root actor in gather unprocessed messages
        tasks = [_get_mailboxes_for_actor_step(object_loader, actor_id, step_id) for actor_id, step_id in actor_heads.items()]
        actor_mailboxes = await asyncio.gather(*tasks)
        mailbox_updates = _find_pending_messages(actor_mailboxes)
        #schedule the updates on the main queue
        for mailbox_update in mailbox_updates:
            await self._event_queue.put(self._RouteMessageEvent.from_mailbox_update(agent_id, mailbox_update))

        logger.info(f"Started agent {agent_id.hex()}: it has {len(actors_without_root)} actors (excl. root) and {len(mailbox_updates)} unprocessed messages.")
    

async def _get_mailboxes_for_actor_step(object_loader:ObjectLoader, actor_id:ActorId, step_id:StepId) -> tuple[ActorId, Mailbox, Mailbox]:
    step:Step = await object_loader.load(step_id)
    if step is None:
        raise Exception(f"Step {step_id.hex()} not found  for actor_id {actor_id.hex()}") #should never happen, would indicate grit being in a bad state
    inbox = (await step.inbox) if step.inbox is not None else {}
    outbox = (await step.outbox) if step.outbox is not None else {}
    return actor_id, inbox, outbox


def _find_pending_messages(actor_mailboxes:list[tuple[ActorId, Mailbox, Mailbox]]) -> list[MailboxUpdate]:
    inboxes = {actor_id: inbox for actor_id, inbox, _ in actor_mailboxes}
    outboxes = {actor_id: outbox for actor_id, _, outbox in actor_mailboxes}
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
