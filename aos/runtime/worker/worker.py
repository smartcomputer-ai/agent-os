from __future__ import annotations
from dataclasses import dataclass, field
import os
import asyncio
from aos.grit import *
from aos.wit import *
from aos.runtime.core import *
from aos.runtime.apex import apex_workers_pb2, apex_workers_pb2_grpc
from aos.runtime.apex.apex_client import ApexClient
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc, agent_store_pb2, agent_store_pb2_grpc
from aos.runtime.store.store_client import StoreClient
from aos.runtime.store.agent_object_store import AgentObjectStore
from aos.runtime.store.agent_references import AgentReferences

import logging
logger = logging.getLogger(__name__)

AgentId = ActorId #the agent is defined by the id of the root actor, so technically, it's an actor id too

@dataclass(slots=True, frozen=True)
class WorkerState:

    capabilities: dict[str, str] = field(default_factory=dict)
    store_clients: dict[str, StoreClient] = field(default_factory=dict)
    assigned_agents: dict[AgentId, apex_workers_pb2.Agent] = field(default_factory=dict)
    runtimes: dict[AgentId, Runtime] = field(default_factory=dict)

async def _worker_loop(
        worker_id:str,
        ticket:str,
        worker_state:WorkerState,
        to_apex_queue:asyncio.Queue[apex_workers_pb2.WorkerToApexMessage],
        to_worker_iterator:AsyncIterator[apex_workers_pb2.WorkerToApexMessage]):
    
    #connect with apex by sending a READY message
    await to_apex_queue.put(apex_workers_pb2.WorkerToApexMessage(
        type=apex_workers_pb2.WorkerToApexMessage.READY,
        worker_id=worker_id,
        ticket=ticket,
        manifest=apex_workers_pb2.WorkerManifest(
            worker_id=worker_id,
            capabilities=worker_state.capabilities,
            current_agents=list(worker_state.assigned_agents.values()))))

    async for message in to_worker_iterator:
        if message.type == apex_workers_pb2.ApexToWorkerMessage.PING:
            logger.info(f"Received ping from apex")
        elif message.type == apex_workers_pb2.ApexToWorkerMessage.GIVE_AGENT:
            await _handle_give_agent(worker_state, message.assignment.agent)


async def _handle_give_agent(worker_state:WorkerState, agent:apex_workers_pb2.Agent):
    agent_id:AgentId = agent.agent_id
    logger.info(f"Received agent {agent_id.hex()} ({agent.agent_did})")
    worker_state.assigned_agents[agent_id] = agent
    #create a store client for the agent
    store_address = agent.store_address
    if store_address not in worker_state.store_clients:
        store_client = StoreClient(store_address)
        await store_client.wait_for_async_channel_ready()
        worker_state.store_clients[store_address] = store_client
    else:
        store_client = worker_state.store_clients[store_address]

    #stores
    object_store = AgentObjectStore(store_client, agent_id)
    references = AgentReferences(store_client, agent_id)

    #create a runtime for the agent
    #TODO: agent_name is not working rn
    runtime = Runtime(object_store, references, agent.agent_did)
    worker_state.runtimes[agent_id] = runtime

    #TODO: use a runtime wrapper (contains task, stores, etc)
    #start the runtime
    #AS TASK
    await runtime.start()


async def run_worker(worker_id:str=None, apex_address:str="localhost:50052"):
    if worker_id is None:
        worker_id = os.getenv("WORKER_ID", None)
        if worker_id is None:
            #create a random worker id
            worker_id = f"worker-{os.urandom(8).hex()}"

    logger.info(f"Starting worker: {worker_id}")

    #outer loop of worker is trying to maintain a connection to the apex server
    #TODO
    client = ApexClient(apex_address)
    await client.wait_for_async_channel_ready()
    worker_stub = client.get_apex_workers_stub_async()

    #register worker
    register_response = await worker_stub.RegisterWorker(apex_workers_pb2.WorkerRegistrationRequest(worker_id=worker_id))
    #the ticket is needed to connect to the apex-worker duplex stream
    ticket = register_response.ticket
    to_apex_queue:asyncio.Queue[apex_workers_pb2.WorkerToApexMessage] = asyncio.Queue()
    worker_state = WorkerState()

    

    #connect to the duplex stream
    stream = worker_stub.ConnectWorker()




if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    asyncio.run(run_worker())
