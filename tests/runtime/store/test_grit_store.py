import asyncio
import os
from aos.grit import *
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc
from aos.runtime.store import agent_store_pb2, agent_store_pb2_grpc
from aos.runtime.store.store_client import StoreClient
from aos.runtime.store.agent_object_store import AgentObjectStore
from aos.runtime.store.store_server import start_server

async def test_read_write(tmp_path):
    server_task = asyncio.create_task(start_server(str(tmp_path)))

    client = StoreClient()
    await client.wait_for_async_channel_ready()

    #create agent
    agent_stub = client.get_agent_store_stub_async()
    agent_response:agent_store_pb2.CreateAgentResponse = await agent_stub.CreateAgent(agent_store_pb2.CreateAgentRequest())
    agent_id = agent_response.agent_id

    #save object
    object_store = AgentObjectStore(client, agent_id)

    blob = Blob({'hi': 'there', 'foo': 'bar'}, os.urandom(1024))

    blob_id = await object_store.store(blob)

    blob2 = await object_store.load(blob_id)
    assert blob.data == blob2.data
    assert blob.headers == blob2.headers
