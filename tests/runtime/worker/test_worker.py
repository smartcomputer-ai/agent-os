import asyncio
import os
from aos.grit import *
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc
from aos.runtime.store import agent_store_pb2, agent_store_pb2_grpc
from aos.runtime.store.store_client import StoreClient
from aos.runtime.store.agent_object_store import AgentObjectStore
from aos.runtime.store.store_server import start_server as start_store_server
from aos.runtime.apex import apex_api_pb2, apex_api_pb2_grpc
from aos.runtime.apex import apex_workers_pb2, apex_workers_pb2_grpc
from aos.runtime.apex.apex_client import ApexClient
from aos.runtime.apex.apex_server import start_server as start_apex_server
from aos.runtime.worker.worker import run_worker

import logging
logging.basicConfig(level=logging.INFO)

#run with:
# poetry run pytest tests/runtime/worker/ --log-cli-level=10 -s

async def test_worker(tmp_path):
    store_server_task = asyncio.create_task(start_store_server(str(tmp_path)))
    apex_server_task = asyncio.create_task(start_apex_server())
    #worker_task = asyncio.create_task(run_worker())

    store_client = StoreClient()
    await store_client.wait_for_async_channel_ready()
    apex_client = ApexClient()
    await apex_client.wait_for_async_channel_ready()

    #create agent
    agent_stub = store_client.get_agent_store_stub_async()
    agent_response:agent_store_pb2.CreateAgentResponse = await agent_stub.CreateAgent(agent_store_pb2.CreateAgentRequest())
    agent_id = agent_response.agent_id
    print("test: agent_id", agent_id.hex())

    #start agent
    apex_api_stub = apex_client.get_apex_api_stub_async()
    await apex_api_stub.StartAgent(apex_api_pb2.StartAgentRequest(agent_id=agent_id))
    print("test: agent started")


    # #save object
    # object_store = AgentObjectStore(store_client, agent_id)

    # blob = Blob({'hi': 'there', 'foo': 'bar'}, os.urandom(1024))

    # blob_id = await object_store.store(blob)

    # blob2 = await object_store.load(blob_id)
    # assert blob.data == blob2.data
    # assert blob.headers == blob2.headers
