# run the grit server

import asyncio
import logging
import time

from aos.grit import *
from aos.wit import *
from . agent_references import AgentReferences
from . grit_store_client import GritStoreClient

async def arun() -> None:
    client = GritStoreClient()
    refs1 = AgentReferences(client, get_random_object_id())
    refs2 = AgentReferences(client, get_random_object_id())

    await refs1.set("ref1", get_random_object_id())
    await refs1.set("ref2", get_random_object_id())
    await refs2.set("ref1", get_random_object_id())
    await refs2.set("ref2", get_random_object_id())

    logging.info(f"refs1: {(await refs1.get('ref1')).hex()}")
    logging.info(f"refs2: {(await refs2.get('ref1')).hex()}")

    logging.info(f"all refs1: {await refs1.get_all()}")

    await client.close()
    logging.info("Done")

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    asyncio.run(arun())
