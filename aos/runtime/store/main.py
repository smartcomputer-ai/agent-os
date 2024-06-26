# run the grit server

import asyncio
import logging
import time

from aos.grit import *
from aos.wit import *
from aos.runtime.store import grit_store_pb2, agent_store_pb2
from . agent_object_store import AgentObjectStore
from .store_client import StoreClient

async def arun() -> None:
    client = StoreClient()

    response:agent_store_pb2.CreateAgentResponse = await client.get_agent_store_stub_async().CreateAgent(agent_store_pb2.CreateAgentRequest())
    
    logging.info(f"Created agent with id {response.agent_id.hex()}")
    logging.info(f"Created agent with point {response.point}")
    agent_id = response.agent_id

    # create a few more agents (just to make sure it all works)
    response2:agent_store_pb2.CreateAgentResponse = await client.get_agent_store_stub_async().CreateAgent(agent_store_pb2.CreateAgentRequest())
    response3:agent_store_pb2.CreateAgentResponse = await client.get_agent_store_stub_async().CreateAgent(agent_store_pb2.CreateAgentRequest())

    #get agents, just to test
    print("=====================================")
    print("Getting agents")
    agent_by_id = await client.get_agent_store_stub_async().GetAgent(agent_store_pb2.GetAgentRequest(agent_id=response.agent_id))
    print(agent_by_id)
    agent_by_point = await client.get_agent_store_stub_async().GetAgent(agent_store_pb2.GetAgentRequest(point=response.point))
    print(agent_by_point)
    all_agents = await client.get_agent_store_stub_async().GetAgents(agent_store_pb2.GetAgentsRequest())
    print(all_agents)

    #set a var and filter by it
    await client.get_agent_store_stub_async().SetVar(agent_store_pb2.SetVarRequest(agent_id=response.agent_id, key="test", value="test"))
    agent_by_var = await client.get_agent_store_stub_async().GetAgents(agent_store_pb2.GetAgentsRequest(var_filters={"test":"test"}))
    print("agent by var", agent_by_var)
    print("=====================================")

    object_store = AgentObjectStore(client, agent_id)

    t1 = time.perf_counter()
    for i in range(100000):
        should_log = i % 1000 == 0

        blob1 = BlobObject.from_str("Hi "+str(i))
        object_id = await blob1.persist(object_store)
        if should_log:
            logging.info(f"Persisted object with id {object_id.hex()}")

        # this is not good for perf testing, because most of the reads will be cached
        # blob2 = await BlobObject.from_blob_id(object_store, object_id)
        # if should_log:
        #     logging.info(f"Loaded object with id {blob2.get_as_str()}")

        if should_log:
            # time elapsed since beginning
            t_snapshot = time.perf_counter()
            logging.info(f"Elapsed time: {t_snapshot-t1:0.2f} seconds")

    t2 = time.perf_counter()
    logging.info(f"Elapsed time: {t2-t1:0.2f} seconds")

    await client.close()
    logging.info("Done")

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    asyncio.run(arun())
