
from abc import ABC
from dataclasses import dataclass
import os
import asyncio
import grpc
from aos.grit import *
from enum import Enum
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc
from aos.runtime.store import secret_store_pb2_grpc, grit_store_pb2_grpc
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
        agent_id:ActorId
        def __init__(self, agent_id:ActorId) -> None:
            super().__init__()
            self.agent_id = agent_id

    class _StopAgentEvent(_EventWithCompletion):
        agent_id:ActorId
        def __init__(self, agent_id:ActorId) -> None:
            super().__init__()
            self.agent_id = agent_id

    class _InjectMessageEvent(_EventWithResult):
        inject_request:apex_api_pb2.RunQueryRequest
        def __init__(self, inject_request:apex_api_pb2.InjectMessageRequest) -> None:
            super().__init__()
            self.inject_request = inject_request

        async def wait_for_result(self, timeout_seconds:float=15)-> apex_api_pb2.InjectMessageResponse:
            return await super().wait_for_result(timeout_seconds)
    

    class _CoreLoopState:
        agents:dict[ActorId, apex_workers_pb2.Agent] = {}
        actors:dict[ActorId, apex_workers_pb2.Actor] = {}

        messages:dict[ActorId, list[apex_api_pb2.Message]] = {}


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
            store_stub = grit_store_pb2_grpc.GritStoreStub(channel)
            secret_stub = secret_store_pb2_grpc.SecretStoreStub(channel)   

            # get all agents
            agents_response:grit_store_pb2.GetAgentsResponse = await store_stub.GetAgents(grit_store_pb2.GetAgentsRequest()) 
            loop_state.agents = {agent_id:apex_workers_pb2.Agent(agent_id=agent_id, agent_did=did, grit_address=self._store_address) for did,agent_id in agents_response.agents.items()}
        
            # gather unprocessed messages
            # TODO

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

