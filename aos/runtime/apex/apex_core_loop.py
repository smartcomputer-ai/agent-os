
from __future__ import annotations
from abc import ABC
from dataclasses import dataclass, field
import os
import random
import asyncio
import grpc
import time
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

#==============================================================
# Documentation
#==============================================================

# Main Steps:
# 1) get all agents from grit
# 2) wait for workers
# 3) assign agents to workers (compare agent's requested capabilities to worker's capabilities)

# The worker is in charge of running the agent. Apex does not route messages unless they are root_actor messages.
# Right now, an agent can only run on a single worker. Later, workers can coordinate to split agent workloads 
# (with wits with different capabilities) and apex will facilitate


#==============================================================
# Type defs and constants
#==============================================================
AgentId = ActorId #the agent is defined by the id of the root actor, so technically, it's an actor id too
AgendDID = str
WorkerId = str
MailboxUpdate = tuple[ActorId, ActorId, MessageId] # sender_id, recipient_id, message_id
TimeSinceUnassigned = float #time since agent was unassigned (using perf_counter)

STORE_APEX_STATUS_VAR_NAME = "apex.status"
STORE_APEX_STATUS_STARTED = "started"
STORE_APEX_STATUS_STOPPED = "stopped"
STORE_CAPABILITIES_VAR_PREFIX = "capabilities" #what capabilities the agent requires


#==============================================================
# Apex State Management Classes
#==============================================================
@dataclass(slots=True)
class WorkerState:
    worker_id:str
    ticket:str
    capabilities:dict[str, str] = field(default_factory=dict) 
    current_agents:set[AgentId] = field(default_factory=set) 

    to_worker_queue:asyncio.Queue[apex_workers_pb2.ApexToWorkerMessage]|None = None

    @property
    def is_connected(self):
        return self.to_worker_queue is not None
    
    def deep_copy(self):
        return WorkerState(
            worker_id=self.worker_id,
            ticket=self.ticket,
            capabilities={k:v for k,v in self.capabilities.items()},
            current_agents={k for k in self.current_agents},
            to_worker_queue=None) #clones do not have access to the worker queue (only the loop has access to it)
    
    def to_apex_api_worker_info(self):
        return apex_api_pb2.WorkerInfo(
            worker_id=self.worker_id,
            capabilities={k:v for k,v in self.capabilities.items()},
            current_agents=list(self.current_agents))

    
@dataclass(slots=True)
class AgentInfo:
    agent_id:AgentId
    agent_did:str
    store_address:str
    capabilities:dict[str, str]

    def deep_copy(self):
        return AgentInfo(
            agent_id=self.agent_id,
            agent_did=self.agent_did,
            store_address=self.store_address,
            capabilities={k:v for k,v in self.capabilities.items()})
    
    def to_apex_api_agent_info(self):
        return apex_api_pb2.AgentInfo(
            agent_id=self.agent_id,
            agent_did=self.agent_did,
            store_address=self.store_address,
            capabilities={k:v for k,v in self.capabilities.items()})
    
    def to_apex_worker_agent(self):
        return apex_workers_pb2.Agent(
            agent_id=self.agent_id,
            agent_did=self.agent_did,
            store_address=self.store_address,
            capabilities={k:v for k,v in self.capabilities.items()})
    

@dataclass(slots=True)
class ApexCoreState:

    is_dirty:bool = False
    workers:dict[WorkerId, WorkerState] = field(default_factory=dict) 
    agents:dict[AgentId, AgentInfo] = field(default_factory=dict) 
    #TODO: keep some sort of info on what the last worker was and how long ago to make agent asssignments 
    #      a bit sticky to avoid too much dirft when workers come in and out, but this is an optimization
    unassigned_agents:dict[AgentId, TimeSinceUnassigned] = field(default_factory=dict)  #are not assigned to a worker
    assigned_agents:dict[AgentId, WorkerId] = field(default_factory=dict)  #are assigned to a worker
    last_rebalance_time:float = 0

    def start_loop(self):
        self.is_dirty = False

    def _mark_dirty(self):
        self.is_dirty = True

    def add_agent(self, agent_info:AgentInfo):
        self._mark_dirty()
        self.agents[agent_info.agent_id] = agent_info
        self.unassigned_agents[agent_info.agent_id] = time.perf_counter()

    def remove_agent(self, agent_id:AgentId):
        self._mark_dirty()
        if agent_id in self.agents:
            del self.agents[agent_id]
        if agent_id in self.assigned_agents:
            worker_id = self.assigned_agents[agent_id]
            del self.assigned_agents[agent_id]
            if agent_id in self.workers[worker_id].current_agents:
                self.workers[worker_id].current_agents.remove(agent_id)
        if agent_id in self.unassigned_agents:
            del self.unassigned_agents[agent_id]
        
    def add_worker(self, worker_id:str, ticket:str):
        self._mark_dirty()
        self.workers[worker_id] = WorkerState(worker_id, ticket)

    def set_worker_connect(self, worker_id:str, capabilities:dict[str, str], to_worker_queue):
        self._mark_dirty()
        self.workers[worker_id].capabilities = {k:v for k,v in capabilities.items()}
        self.workers[worker_id].to_worker_queue = to_worker_queue

    def assign_agent_to_worker(self, agent_id:AgentId, worker_id:WorkerId):
        self._mark_dirty()
        #must be in unassigned_agents
        if agent_id in self.assigned_agents:
            raise ValueError(f"Agent {agent_id} is already assigned to worker {self.assigned_agents[agent_id]}")
        if agent_id not in self.unassigned_agents:
            raise ValueError(f"Agent {agent_id} is not in unassigned_agents")
        self.assigned_agents[agent_id] = worker_id
        del self.unassigned_agents[agent_id]
        self.workers[worker_id].current_agents.add(agent_id)

    def unassign_agent_from_worker(self, agent_id:AgentId):
        self._mark_dirty()
        if agent_id not in self.assigned_agents:
            raise ValueError(f"Agent {agent_id} is not assigned to any worker.")
        worker_id = self.assigned_agents[agent_id]
        del self.assigned_agents[agent_id]
        self.unassigned_agents[agent_id] = time.perf_counter()
        self.workers[worker_id].current_agents.remove(agent_id)

    def remove_worker(self, worker_id:str):
        self._mark_dirty()
        for agent_id in self.workers[worker_id].current_agents:
            self.unassigned_agents[agent_id] = time.perf_counter()
            del self.assigned_agents[agent_id]
        del self.workers[worker_id]

    def deep_copy(self):
        return ApexCoreState(
            is_dirty=False,
            workers={k:v.deep_copy() for k,v in self.workers.items()},
            agents={k:v.deep_copy() for k,v in self.agents.items()},
            unassigned_agents={k:v for k,v in self.unassigned_agents.items()},
            assigned_agents={k:v for k,v in self.assigned_agents.items()})
    

#==============================================================
# Util Classes (for events below)
#==============================================================
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


#==============================================================
# Apex Core Loop Class
# Implmented as a single, asynchronous loop
# all interactions with the loop are through events
#==============================================================
class ApexCoreLoop:

    @dataclass(frozen=True, slots=True)
    class _RegisterWorkerEvent:
        worker_id:str
        ticket:str

    @dataclass(frozen=True, slots=True)
    class _WorkerConnectedEvent:
        worker_id:WorkerId
        ticket:str #used to verify that the handshake was correct
        manifest:apex_workers_pb2.WorkerManifest
        to_worker_queue:asyncio.Queue[apex_workers_pb2.ApexToWorkerMessage]

    @dataclass(frozen=True, slots=True)
    class _WorkerDisconnectedEvent:
        worker_id:WorkerId

    @dataclass(frozen=True, slots=True)
    class _RebalanceAgentsEvent:
        new_workers:list[WorkerId]|None = None
        removed_workers:list[WorkerId]|None = None
        desired_agents:dict[AgentId, WorkerId]|None = None

    # @dataclass(frozen=True, slots=True)
    # class _RouteMessageEvent:
    #     worker_id:WorkerId|None #from which which worker, if None is generate by apex
    #     message:apex_workers_pb2.ActorMessage

    #     @property
    #     def is_to_root_actor(self):
    #         return self.message.recipient_id == self.message.agent_id

    #     @classmethod
    #     def from_mailbox_update(cls, agent_id:AgentId, mailbox_update:MailboxUpdate):
    #         return cls(worker_id=None, message=apex_workers_pb2.ActorMessage(
    #             agent_id=agent_id,
    #             sender_id=mailbox_update[0],
    #             recipient_id=mailbox_update[1],
    #             message_id=mailbox_update[2]))

    # @dataclass(frozen=True, slots=True)
    # class _RouteQueryEvent:
    #     worker_id:WorkerId
    #     query:apex_workers_pb2.ActorQuery

    # @dataclass(frozen=True, slots=True)
    # class _RouteQueryResultEvent:
    #     worker_id:WorkerId
    #     query_result:apex_workers_pb2.ActorQueryResult

    # class _RunQueryEvent(_EventWithResult):
    #     query:apex_api_pb2.RunQueryRequest

    #     def __init__(self, query:apex_api_pb2.RunQueryRequest) -> None:
    #         super().__init__()
    #         self.query = query

    #     async def wait_for_result(self, timeout_seconds:float=90)-> apex_api_pb2.RunQueryResponse:
    #         return await super().wait_for_result(timeout_seconds)
    
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

    class _InjectMessageEvent(_EventWithCompletion):
        agent_id:AgentId
        inject_request:apex_api_pb2.InjectMessageRequest
        def __init__(self, inject_request:apex_api_pb2.InjectMessageRequest) -> None:
            super().__init__()
            self.agent_id = inject_request.agent_id
            self.inject_request = inject_request

        def to_message_injection(self):
            if self.inject_request.message_data is not None:
                message_data = apex_workers_pb2.InjectMessageData(
                    headers={k:v for k,v in self.inject_request.message_data.headers.items()},
                    is_signal=self.inject_request.message_data.is_signal)
                if self.inject_request.message_data.content_id is not None:
                    message_data.content_id = self.inject_request.message_data.content_id
                elif self.inject_request.message_data.content_blob is not None:
                    message_data.content_blob = self.inject_request.message_data.content_blob
            else:
                message_data = None
            injection = apex_workers_pb2.MessageInjection(
                agent_id=self.agent_id,
                recipient_id=self.inject_request.recipient_id)
            if message_data is not None:
                injection.message_data = message_data
            else:
                injection.message_id = self.inject_request.message_id
            return injection
                
    

        
        # these kinds of things need to go a "emphemeral state" class
        #unprocessed_messages:dict[ActorId, list[MailboxUpdate]] = {} #the recipient actor doesn't have a worker yet
        #unporcessed_queries
        #pending_queries (for callback and result routing)


    def __init__(
            self,
            store_address:str,
            node_id:str|None=None,
            assign_time_delay_secods:float=0,
            ) -> None:
        
        if node_id is None:
            #create a random node id
            node_id = os.urandom(8).hex()

        self._node_id = node_id
        self._store_address = store_address
        self._assign_time_delay_secods = assign_time_delay_secods
        self._rebalance_interval_seconds = 5
        self._cancel_event = asyncio.Event()
        self._running_event = asyncio.Event()
        self._event_queue = asyncio.Queue()
        self._state_copy = None
        self._state_copy_lock = asyncio.Lock()


    #==============================================================
    # Main Loop
    #==============================================================
    async def _make_state_copy(self, loop_state:ApexCoreState):
        async with self._state_copy_lock:
            self._state_copy = loop_state.deep_copy()

    async def get_state_copy(self) -> ApexCoreState:
        async with self._state_copy_lock:
            return self._state_copy
        
    async def wait_until_running(self):
        await self._running_event.wait()

    def stop(self):
        self._cancel_event.set()
        
    async def start(self):
        logger.info("Starting apex core loop")
        loop_state = ApexCoreState()
        #try to connect to the store server
        store_client = StoreClient(self._store_address)
        await store_client.wait_for_async_channel_ready(timeout_seconds=30*60) #30 minutes
        # gather started agents, their actors, and unprocessed messages
        await self._gather_started_agents(loop_state, store_client)
        #first copy of the state
        await self._make_state_copy(loop_state)        
        #start processing of main loop
        await asyncio.sleep(0) #yield to allow other tasks to run
        self._running_event.set() #signal that apex is about to start
        while not self._cancel_event.is_set():
            loop_state.start_loop()
            event = None
            try:
                event = await asyncio.wait_for(self._event_queue.get(), 0.05)
            except asyncio.TimeoutError:
                if time.perf_counter() - loop_state.last_rebalance_time > self._rebalance_interval_seconds:
                    await self._event_queue.put(self._RebalanceAgentsEvent())
                continue #test for cancel (in the while condition) and try again
            
            if isinstance(event, self._RegisterWorkerEvent):
                await self._handle_register_worker(event, loop_state)

            elif isinstance(event, self._WorkerConnectedEvent):
                await self._handle_worker_connected(event, loop_state)

            elif isinstance(event, self._WorkerDisconnectedEvent):
                await self._handle_worker_disconnected(event, loop_state)

            elif isinstance(event, self._RebalanceAgentsEvent):
                await self._handle_rebalance_agents(event, loop_state)

            elif isinstance(event, self._StartAgentEvent):
                await self._handle_start_agent(event, loop_state, store_client)

            elif isinstance(event, self._StopAgentEvent):
                await self._handle_stop_agent(event, loop_state, store_client)

            elif isinstance(event, self._InjectMessageEvent):
                await self._handle_inject_message(event, loop_state)

            else:
                logger.warning(f"Apex core loop: Unknown event type {type(event)}.")

            if loop_state.is_dirty:
                await self._make_state_copy(loop_state)

        logger.info("Apex core loop stopped.")
        #cleanup
        await store_client.close()


    #==============================================================
    # Event Handlers
    #==============================================================
    async def _handle_register_worker(self, event:_RegisterWorkerEvent, loop_state:ApexCoreState):
        #if there is an existing worker with the same id, disconnect it
        if event.worker_id in loop_state.workers:
            worker_state = loop_state.workers[event.worker_id]
            if worker_state.is_connected:
                logger.warning(f"RegisterWorkerEvent: Worker {event.worker_id} is already connected, disconnecting it.")
                worker_state.to_worker_queue.put_nowait(None)
        #add worker with new ticket
        loop_state.add_worker(event.worker_id, event.ticket)
        logger.info(f"RegisterWorkerEvent: Worker {event.worker_id} with ticket {event.ticket} registered.")


    async def _handle_worker_connected(self, event:_WorkerConnectedEvent, loop_state:ApexCoreState):
        if event.worker_id not in loop_state.workers:
            logger.warning(f"WorkerConnectedEvent: Worker {event.worker_id} trying to connect, but it is not registered.")
        elif loop_state.workers[event.worker_id].ticket != event.ticket:
            logger.warning(f"WorkerConnectedEvent: Worker {event.worker_id} trying to connect with wrong ticket, closing connection.")
            event.to_worker_queue.put_nowait(None)
        elif event.manifest.current_agents is not None and len(event.manifest.current_agents) > 0:
            logger.error(f"WorkerConnectedEvent: Worker {event.worker_id} sent current agents ({event.manifest.current_agents}), but this is not allowed on connect, must be empty, see proto file.")
            event.to_worker_queue.put_nowait(None)
            loop_state.remove_worker(event.worker_id)
        else:
            loop_state.set_worker_connect(event.worker_id, event.manifest.capabilities, event.to_worker_queue)

            #see if the worker requested any agents to run
            desired_agent_ids = None
            if event.manifest.desired_agents is not None:
                desired_agent_ids = {agent.agent_id:event.worker_id for agent in event.manifest.desired_agents}

            #add rebalance event since there is now a new worker
            await self._event_queue.put(self._RebalanceAgentsEvent(
                new_workers=[event.worker_id], 
                desired_agents=desired_agent_ids))
            logger.info(f"WorkerConnectedEvent: Worker {event.worker_id} connected.")


    async def _handle_worker_disconnected(self, event:_WorkerDisconnectedEvent, loop_state:ApexCoreState):
        if event.worker_id not in loop_state.workers:
            logger.warning(f"WorkerDisconnectedEvent: Worker {event.worker_id} trying to disconnect, but it is not registered, NO-OP.")
        else:
            worker_state = loop_state.workers[event.worker_id]
            if worker_state.is_connected:
                #this might be important, because it allows the the server request processing task to terminate too
                worker_state.to_worker_queue.put_nowait(None)
            loop_state.remove_worker(event.worker_id)
            #add rebalance event since there is now one worker less
            await self._event_queue.put(self._RebalanceAgentsEvent(removed_workers=[event.worker_id]))
            logger.info(f"WorkerDisconnectedEvent: Worker {event.worker_id} disconnected.")
        

    async def _handle_rebalance_agents(self, event:_RebalanceAgentsEvent, loop_state:ApexCoreState):
        # in this first version, we'll just assign the agents to the workers that have the least agents
        # this is not optimal, but it's a start
        workers = [w for w in loop_state.workers.values() if w.is_connected]
        agents_to_assign = list(loop_state.unassigned_agents.keys())

        if len(agents_to_assign) > 0:
            logger.info(f"RebalanceAgentsEvent: Rebalancing {len(agents_to_assign)} agents to {len(workers)} workers.")

        async def give_agent(agent_id, worker_id):
            loop_state.assign_agent_to_worker(agent_id, worker_id)
            await loop_state.workers[worker_id].to_worker_queue.put(apex_workers_pb2.ApexToWorkerMessage(
                type=apex_workers_pb2.ApexToWorkerMessage.GIVE_AGENT,
                assignment=apex_workers_pb2.AgentAssignment(
                    agent_id=agent_id, 
                    agent=loop_state.agents[agent_id].to_apex_worker_agent())))
            logger.info(f"RebalanceAgentsEvent: Agent {agent_id.hex()} assigned to worker {selected_worker_id}.")
        
        for agent_id in agents_to_assign:
            #if the worker requested the agent, assign it right away (greedy)
            if event.desired_agents is not None and agent_id in event.desired_agents:
                selected_worker_id = event.desired_agents[agent_id]
                await give_agent(agent_id, selected_worker_id)                
                continue

            #if the time since unassigned is too short, skip
            if time.perf_counter() - loop_state.unassigned_agents[agent_id] < self._assign_time_delay_secods:
                continue

            #TODO: find the workers that match the capabilities of the agent
            matching_workers = workers
            if len(matching_workers) == 0:
                logger.warning(f"RebalanceAgentsEvent: No workers available to assign agent {agent_id.hex()}.")
                continue

            #find the worker with the least agents
            selected_worker_id = min(matching_workers, key=lambda w: len(w.current_agents)).worker_id
            await give_agent(agent_id, selected_worker_id)                

        loop_state.last_rebalance_time = time.perf_counter()


    async def _handle_start_agent(self, event:_StartAgentEvent, loop_state:ApexCoreState, store_client:StoreClient):
        #check if already running
        if event.agent_id in loop_state.agents:
            logger.warning(f"StartAgentEvent: Agent {event.agent_id.hex()} is already running, NO-OP.")
        else:
            agent_id = event.agent_id
            logger.info(f"StartAgentEvent: Starting agent {agent_id.hex()}.")
            agent_store = store_client.get_agent_store_stub_async()
            #get agent from the store
            agent_response:agent_store_pb2.GetAgentResponse = await agent_store.GetAgent(agent_store_pb2.GetAgentRequest(agent_id=agent_id))
            if not agent_response.exists:
                logger.error(f"StartAgentEvent: Agent {agent_id.hex()} does not exist in the agent store.")
                event.set_completion()
            else:
                #mark agent as started in the agent store
                await store_client.get_agent_store_stub_async().SetVar(
                    agent_store_pb2.SetVarRequest(
                        agent_id=agent_id,
                        key=STORE_APEX_STATUS_VAR_NAME,
                        value=STORE_APEX_STATUS_STARTED))
                #gather actors and unpocessed messages

                _, agent_capabilities = await self._gather_agent_capabilities(agent_id, store_client)
                agent_info = AgentInfo(
                    agent_id=agent_id,
                    agent_did=agent_response.agent_did,
                    store_address=self._store_address,
                    capabilities=agent_capabilities)
                loop_state.add_agent(agent_info)
                logger.info(f"StartAgentEvent: Agent {agent_id.hex()} ({agent_info.agent_did}) started.")
            #rebalance
            await self._event_queue.put(self._RebalanceAgentsEvent())
        event.set_completion()


    async def _handle_stop_agent(self, event:_StopAgentEvent, loop_state:ApexCoreState, store_client:StoreClient):
        #check if already stopped
        if event.agent_id not in loop_state.agents:
            logger.warning(f"StopAgentEvent: Agent {event.agent_id.hex()} is not running, NO-OP.")
        else:
            agent_id = event.agent_id
            logger.info(f"StopAgentEvent: Stopping agent {agent_id.hex()}.")
            #send YANK message to worker (if there is one)
            if agent_id in loop_state.assigned_agents:
                worker_id = loop_state.assigned_agents[agent_id]
                worker_state = loop_state.workers[worker_id]
                await worker_state.to_worker_queue.put(apex_workers_pb2.ApexToWorkerMessage(
                    type=apex_workers_pb2.ApexToWorkerMessage.YANK_AGENT,
                    assignment=apex_workers_pb2.AgentAssignment(agent_id=agent_id)))
                loop_state.unassign_agent_from_worker(agent_id)
            #TODO: consider giving the worker some time to stop, espcially if the the stopped state in the store errors the worker (not the case rn)
            #mark agent as stopped in the agent store
            await store_client.get_agent_store_stub_async().SetVar(
                agent_store_pb2.SetVarRequest(
                    agent_id=agent_id,
                    key=STORE_APEX_STATUS_VAR_NAME,
                    value=STORE_APEX_STATUS_STOPPED))
            loop_state.remove_agent(agent_id)
            
        event.set_completion()

    
    async def _handle_inject_message(self, event:_InjectMessageEvent, loop_state:ApexCoreState):
        #check if the target agent is running
        if event.agent_id not in loop_state.agents or event.agent_id not in loop_state.assigned_agents:
            logger.warning(f"InjectMessageEvent: Agent {event.agent_id.hex()} is not running or assigned, cannot inject message for actor {event.inject_request.recipient_id.hex()}.")
        else:
            logger.info(f"InjectMessageEvent: Injecting message for agent {event.agent_id.hex()} to actor {event.inject_request.recipient_id.hex()}.")
            #route the message to the worker that is running the agent
            worker_state = loop_state.workers[loop_state.assigned_agents[event.agent_id]]
            await worker_state.to_worker_queue.put(event.to_message_injection())
        event.set_completion()


    #==============================================================
    # Apex interaction APIs 
    # Works by injecting events into the main loop
    #==============================================================
    def _ensure_running(self):
        if not self._running_event.is_set() or self._cancel_event.is_set():
            raise RuntimeError("Apex core loop is not running.")
 

    async def start_agent(self, agent_id:AgentId):
        self._ensure_running()
        event = self._StartAgentEvent(agent_id)
        await self._event_queue.put(event)
        await event.wait_for_completion(timeout_seconds=10)


    async def stop_agent(self, agent_id:AgentId):
        self._ensure_running()
        event = self._StopAgentEvent(agent_id)
        await self._event_queue.put(event)
        await event.wait_for_completion(timeout_seconds=10)


    async def register_worker(self, worker_id:str) -> str:
        """Register a worker with the apex core loop. Returns a ticket that the worker must use to connect."""
        self._ensure_running()
        ticket = os.urandom(8).hex()
        event = self._RegisterWorkerEvent(worker_id, ticket)
        await self._event_queue.put(event)
        return ticket
    

    async def worker_connected(
            self, 
            worker_id:str, 
            ticket:str, 
            manifest:apex_workers_pb2.WorkerManifest,
            to_worker_queue:asyncio.Queue[apex_workers_pb2.ApexToWorkerMessage],):
        self._ensure_running()
        event = self._WorkerConnectedEvent(worker_id, ticket, manifest, to_worker_queue)
        await self._event_queue.put(event)
    

    async def worker_disconnected(self, worker_id:str):
        self._ensure_running()
        event = self._WorkerDisconnectedEvent(worker_id)
        await self._event_queue.put(event)


    async def inject_message(self, inject_request:apex_api_pb2.InjectMessageRequest) -> None:
        self._ensure_running()
        event = self._InjectMessageEvent(inject_request)
        await self._event_queue.put(event)
        await event.wait_for_completion(timeout_seconds=5)


    #==============================================================
    # Helpers
    # E.g., get info about agents, actors, etc.
    #==============================================================
    async def _gather_started_agents(
            self, 
            loop_state:ApexCoreState,
            store_client:StoreClient,):
        #get all agents that have a "started" status
        agents_response:agent_store_pb2.GetAgentsResponse = await (
            store_client
            .get_agent_store_stub_async()
            .GetAgents(agent_store_pb2.GetAgentsRequest(var_filters={STORE_APEX_STATUS_VAR_NAME: STORE_APEX_STATUS_STARTED})) 
        )
        agent_id_to_did = {agent_id:did for did, agent_id in agents_response.agents.items()}
        tasks = [self._gather_agent_capabilities(agent_id, store_client) for agent_id in agent_id_to_did.keys()]
        agent_capabilities = await asyncio.gather(*tasks)
        agent_capabilities_lookup = {agent_id:capabilities for agent_id, capabilities in agent_capabilities}

        for agent_id, did in agent_id_to_did.items():
            agent_info = AgentInfo(
                agent_id=agent_id,
                agent_did=did,
                store_address=self._store_address,
                capabilities=agent_capabilities_lookup[agent_id])
            loop_state.add_agent(agent_info)

        logger.info(f"Found {len(loop_state.agents)} agents with status 'started'.")


    async def _gather_agent_capabilities(self, agent_id:AgentId, store_client:StoreClient) -> tuple[AgentId, dict[str, str]]:
        #get the agent's capabilities
        #TODO: get from actors (??)
        # for now, get from agent store vars
        response:agent_store_pb2.GetVarsResponse = await store_client.get_agent_store_stub_async().GetVars(
                agent_store_pb2.GetVarsRequest(
                    agent_id=agent_id,
                    key_prefix=STORE_CAPABILITIES_VAR_PREFIX))
        return agent_id, {var_name:var_value for var_name, var_value in response.vars.items()}



