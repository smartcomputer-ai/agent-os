# run the grit server

import asyncio
import logging
import time

from aos.grit import *
from aos.wit import *
from . agent_object_store import AgentObjectStore
from . grit_store_client import GritStoreClient

async def arun() -> None:
    client = GritStoreClient()
    object_store = AgentObjectStore(client, get_random_object_id())

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
