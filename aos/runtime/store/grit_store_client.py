from __future__ import print_function
import asyncio
import grpc
from concurrent import futures
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc

import logging
logger = logging.getLogger(__name__)

# the idea is that only one of these clients exists and then the object store and refs classes create one off stubs

class GritStoreClient:
    def __init__(self, server_address="localhost:50051"):
        self.server_address = server_address
        # the async and sync api cannot be shared
        # however, opening two channels is okay, because, appratenly, there is something called a "sun channel"
        # wich is a shared resource between the two client channels (if their configuration is the same)
        # see: https://stackoverflow.com/a/62761510 (last comment)
        self.channel_sync = grpc.insecure_channel(self.server_address)
        self.channel_async = grpc.aio.insecure_channel(self.server_address)

    def get_channel_sync(self):
        return self.channel_sync
    
    def get_channel_async(self):
        return self.channel_async
    
    def get_store_stub_sync(self):
        return grit_store_pb2_grpc.GritStoreStub(self.channel_sync)
    
    def get_store_stub_async(self):
        return grit_store_pb2_grpc.GritStoreStub(self.channel_async)

    async def close(self, grace_period=1.0):
        await self.channel_async.close(grace_period)
        self.channel_sync.close()
