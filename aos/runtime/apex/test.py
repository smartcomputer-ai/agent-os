# run the grit server

import asyncio
import logging
import time
import grpc
from typing import AsyncIterable

from aos.grit import *
from aos.runtime.apex import apex_workers_pb2
from aos.wit import *
from .apex_client import ApexClient

async def arun() -> None:
    client = ApexClient()

    worker_stub = client.get_apex_workers_stub_async()

    async def generate_messages() -> AsyncIterable[apex_workers_pb2.WorkerToApexMessage]:
        while True:
            yield apex_workers_pb2.WorkerToApexMessage(
                worker_id="worker1", 
                type=apex_workers_pb2.WorkerToApexMessage.PING)
            print("sent ping")
            await asyncio.sleep(1)

    apex_stream:AsyncIterable[apex_workers_pb2.ApexToWorkerMessage] = worker_stub.ConnectWorker(generate_messages())

    try:
        async for message in apex_stream:
            print("received apex message", message.type)
        print("apex stream done")
    except grpc.aio.AioRpcError as e:
        if e.code() == grpc.StatusCode.CANCELLED:
            print("apex stream cancelled")
        else:
            raise e

    await client.close()
    logging.info("Done")

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    asyncio.run(arun())
