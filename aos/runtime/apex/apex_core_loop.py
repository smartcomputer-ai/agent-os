
from __future__ import annotations
from abc import ABC
from dataclasses import dataclass
import os
import asyncio
import grpc
from aos.grit import *
from enum import Enum
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc
from aos.runtime.store import agent_store_pb2, agent_store_pb2_grpc
from aos.runtime.apex import apex_api_pb2, apex_workers_pb2

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

class ApexCoreLoop:

    @dataclass(frozen=True, slots=True)
    class _RegisterWorkerEvent:
        worker_id:str
        manifest:apex_workers_pb2.WorkerManifest
        ticket:str

    @dataclass(frozen=True, slots=True)
    class _WorkerConnectedEvent:
        worker_id:str
        ticket:str #used to verify that the handshake was correct
        to_worker_queue:asyncio.Queue[apex_workers_pb2.ApexToWorkerMessage]

    @dataclass(frozen=True, slots=True)
    class _WorkerDisconnectedEvent:
        worker_id:str

    @dataclass(frozen=True, slots=True)
    class _RouteMessageEvent:
        worker_id:str #from which which worker
        message:apex_workers_pb2.ActorMessage

    @dataclass(frozen=True, slots=True)
    class _RouteQueryEvent:
        worker_id:str
        query:apex_workers_pb2.ActorQuery

    @dataclass(frozen=True, slots=True)
    class _RouteQueryResultEvent:
        worker_id:str
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

        @property
        def is_connected(self):
            return self.to_worker_queue is not None

    @dataclass(slots=True)
    class _ActorInfo:
        manifest:str

    class _CoreLoopState:
        running_agents:dict[AgentId, AgendDID] = {}
        unassigned_actors:set[ActorId] = {} #are not assigned to a worker
        assigned_actors:dict[ActorId, WorkerId] = {} #are assigned to a worker

        actors:dict[ActorId, ApexCoreLoop._ActorInfo] = {}
        workers:dict[WorkerId, ApexCoreLoop._WorkerState] = {}

        #messages:dict[ActorId, list[]] = {}


    #external signaling
    _cancel_event:asyncio.Event
    _running_event:asyncio.Event

    #internal state
    _event_queue:asyncio.Queue[any] = None #event object

    
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
        
        async with self._connect_to_store_loop() as channel:
            grit_store_stub = grit_store_pb2_grpc.GritStoreStub(channel)
            agent_store_stub = agent_store_pb2_grpc.AgentStoreStub(channel)   

            # get all agents
            agents_response:agent_store_pb2.GetAgentsResponse = await agent_store_stub.GetAgents(agent_store_pb2.GetAgentsRequest(var_filters={"apex.status","started"})) 
            loop_state.running_agents = {agent_id:did for did, agent_id in agents_response.agents.items()}
        
            # gather unprocessed messages
            # TODO

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

                elif isinstance(event, self._WorkerConnectedEvent):
                    if event.worker_id not in loop_state.workers:
                        logger.warn(f"WorkerConnectedEvent: Worker {event.worker_id} trying to connect, but it is not registered, NO-OP.")
                        event.to_worker_queue.put_nowait(None)
                    else:
                        loop_state.workers[event.worker_id].to_worker_queue = event.to_worker_queue
                        logger.info(f"WorkerConnectedEvent: Worker {event.worker_id} connected.")
                        #TODO: assign actors to worker, consider existing manifest
                        # run algo that figures out which workers get which actors, 
                        # rebalance, send relevant GIVE and YANK messages
                        # in the first version, implement a greedy algo that assigns all actors to workers

                elif isinstance(event, self._WorkerDisconnectedEvent):
                    logger.info(f"WorkerDisconnectedEvent: Worker {event.worker_id} disconnected.")
                    #move workers from state

    async def stop(self):
        pass


    async def _connect_to_store_loop(self):
        logger.info("Connecting to store server...")
        tries = 0
        max_tries = 100
        while True:
            tries += 1
            try:
                channel = grpc.aio.insecure_channel(self._store_address)
                await channel.channel_ready()
                logger.info("Connected to store server")
                return channel
            except Exception as e:
                if tries >= max_tries:
                    logger.error(f"Max tries reached, giving up")
                    raise e
                else:
                    logger.warn(f"Was not able to connect to store server {self._store_address}, will try again: {e}")
                    await asyncio.sleep(5)


async def core_loop():
    pass

