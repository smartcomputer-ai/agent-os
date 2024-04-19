from __future__ import annotations
from dataclasses import dataclass, field
import os
import asyncio
import time
import grpc
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
    runtime_tasks: dict[AgentId, asyncio.Task] = field(default_factory=dict)

    
async def run_worker(
        worker_id:str=None, 
        apex_address:str="localhost:50052",
        cancel_token:asyncio.Event=None
        ):
    """Run's a worker that connects to the apex server and runs agents.
    
    How is the worker cancelled/closed?
    When the cancellation token is set, to_apex_iterator terminates, which results in the worker->apex stream to be closed.
    On the apex server side, this is also detected and the worker is unregistered and cleaned up, which results in the apex->worker stream to be closed,
    which terminates the async iterator in the _worker_loop function.
    """
    
    # see if a worker id needs to be generated
    if worker_id is None:
        worker_id = os.getenv("WORKER_ID", None)
        if worker_id is None:
            #create a random worker id
            worker_id = f"worker-{os.urandom(8).hex()}"

    logger.info(f"Starting worker: {worker_id}")

    # if not cancel token is provided, create one
    if cancel_token is None:
        cancel_token = asyncio.Event()

    previously_assigned_agents:list[AgentId]|None = None
    #outer loop of worker is trying to maintain a connection to the apex server
    
    while not cancel_token.is_set():
        #todo: but connect into loop
        client = await ApexClient.get_connected_client_with_retry(apex_address, logger=logger)
        worker_stub = client.get_apex_workers_stub_async()

        try:
            #register worker
            register_response = await worker_stub.RegisterWorker(apex_workers_pb2.WorkerRegistrationRequest(worker_id=worker_id))
            #the ticket is needed to connect to the apex-worker duplex stream
            ticket = register_response.ticket
            to_apex_queue:asyncio.Queue[apex_workers_pb2.WorkerToApexMessage] = asyncio.Queue()


            #connect to the duplex stream
            logger.info(f"Worker: Connecting to apex...")
            to_worker_iterator:AsyncIterator[apex_workers_pb2.ApexToWorkerMessage] = worker_stub.ConnectWorker(
                queue_to_cancellable_async_iterator(to_apex_queue, cancel_token))
            logger.info(f"Worker: Connected to apex, starting worker loop.")
            previously_assigned_agents = await _worker_loop(
                worker_id,
                ticket,
                to_apex_queue,
                to_worker_iterator,
                previously_assigned_agents)
        except grpc.aio.AioRpcError as e:
            logger.warning(f"Worker: Connection to apex failed: {e.code()}: {e.details()}")
        except Exception as e:
            logger.error(f"Worker: Error in worker loop: {e}")
            raise
        
        #this will close the iterator to the apex server
        await to_apex_queue.put(None)
        await client.close()
        await asyncio.sleep(0.5)
        logger.info(f"Worker: Apex connection was closed.")


async def _worker_loop(
        worker_id:str,
        ticket:str,
        to_apex_queue:asyncio.Queue[apex_workers_pb2.WorkerToApexMessage],
        to_worker_iterator:AsyncIterator[apex_workers_pb2.ApexToWorkerMessage],
        previously_assigned_agents:list[AgentId]|None = None,
        ):
    
    worker_state = WorkerState()

    #connect with apex by sending a READY message
    await to_apex_queue.put(apex_workers_pb2.WorkerToApexMessage(
        type=apex_workers_pb2.WorkerToApexMessage.READY,
        worker_id=worker_id,
        ticket=ticket,
        manifest=apex_workers_pb2.WorkerManifest(
            worker_id=worker_id,
            capabilities=worker_state.capabilities,
            desired_agents=previously_assigned_agents)))

    #this iterator will end when the connection to the apex server is closed
    try:
        async for message in to_worker_iterator:
            if message.type == apex_workers_pb2.ApexToWorkerMessage.PING:
                logger.info(f"Worker: Received ping from apex")
            elif message.type == apex_workers_pb2.ApexToWorkerMessage.GIVE_AGENT:
                await _handle_give_agent(worker_state, message.assignment.agent)
            elif message.type == apex_workers_pb2.ApexToWorkerMessage.YANK_AGENT:
                await _handle_yank_agent(worker_state, message.assignment.agent_id)
    except grpc.aio.AioRpcError as e:
        logger.warning(f"Worker: Connection to apex was closed: {e.code()}: {e.details()}")
    except Exception as e:
        logger.error(f"Worker: Error in worker loop: {e}")
        raise
    finally:
        logger.info(f"Worker: Cleaning up worker state...")
        assigned_agent_ids = list(worker_state.assigned_agents.keys())
        #TODO cleanup worker state
        # close all runtimes, etc

        #so the worker can request them in the next connect loop again
        return assigned_agent_ids


async def _handle_give_agent(worker_state:WorkerState, agent:apex_workers_pb2.Agent):
    agent_id:AgentId = agent.agent_id
    logger.info(f"Worker: Received agent {agent_id.hex()} ({agent.agent_did}), will run it...")
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

    runtime_task = asyncio.create_task(runtime.start())
    worker_state.runtime_tasks[agent_id] = runtime_task
    logger.info(f"Worker: Started runtime for {agent_id.hex()} ({agent.agent_did}).")


async def _handle_yank_agent(worker_state:WorkerState, agent_id:AgentId):
    if agent_id not in worker_state.assigned_agents:
        logger.error(f"Worker: Tried to yank agent {agent_id.hex()}, but it is currently not running on this worker.")
        return
    
    logger.info(f"Worker: Yanking agent {agent_id.hex()}, will stop it...")

    runtime = worker_state.runtimes[agent_id]
    runtime_task = worker_state.runtime_tasks[agent_id]
    runtime.stop()
    try:
        await asyncio.wait_for(runtime_task, 2.0)
    except asyncio.TimeoutError as e:
        raise Exception(f"Worker: timeout while stopping runtime for agent {agent_id.hex()}.") from e

    del worker_state.runtime_tasks[agent_id]
    del worker_state.runtimes[agent_id]
    del worker_state.assigned_agents[agent_id]
    logger.info(f"Worker: Stopped runtime for {agent_id.hex()}.")

#convert the queue to an iterator (which is what the gRPC api expects)
# cancel the queue/iterator by adding a None item or by setting the cancel event
async def queue_to_cancellable_async_iterator(queue:asyncio.Queue, cancel:asyncio.Event) -> AsyncIterator:
    while not cancel.is_set():
        event = None
        try:
            event = await asyncio.wait_for(queue.get(), 0.05)
            if event is None:
                break
            yield event
        except asyncio.TimeoutError:
            continue #test for cancel (in the while condition) and try again


if __name__ == "__main__":
    import signal
    import sys

    cancel_token = asyncio.Event()
    def sig_cancel(sig, frame):
        logger.info(f"Received signal ({sig}), will cancel worker.")
        cancel_token.set()
        time.sleep(0.2)
        sys.exit(sig)
        #give it a short while
        
    signal.signal(signal.SIGINT, sig_cancel)
    signal.signal(signal.SIGTERM, sig_cancel)
    logging.basicConfig(level=logging.INFO)
    asyncio.run(run_worker(cancel_token=cancel_token))
