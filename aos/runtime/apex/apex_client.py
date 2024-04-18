from __future__ import print_function
import asyncio
import grpc
from concurrent import futures
from aos.runtime.apex import apex_api_pb2, apex_api_pb2_grpc
from aos.runtime.apex import apex_workers_pb2, apex_workers_pb2_grpc

import logging
logger = logging.getLogger(__name__)

class ApexClient:
    def __init__(self, server_address="localhost:50052"):
        self.server_address = server_address
        # the async and sync api cannot be shared
        # however, opening two channels is okay, because, appratenly, there is something called a "sun channel"
        # wich is a shared resource between the two client channels (if their configuration is the same)
        # see: https://stackoverflow.com/a/62761510 (last comment)
        self.channel_sync = grpc.insecure_channel(self.server_address)
        self.channel_async = grpc.aio.insecure_channel(self.server_address)

    async def wait_for_async_channel_ready(self):
        await self.channel_async.channel_ready()

    def get_channel_sync(self):
        return self.channel_sync
    
    def get_channel_async(self):
        return self.channel_async
    
    def get_apex_api_stub_sync(self):
        return apex_api_pb2_grpc.ApexApiStub(self.channel_sync)
    
    def get_apex_api_stub_async(self):
        return apex_api_pb2_grpc.ApexApiStub(self.channel_async)
    
    def get_apex_workers_stub_sync(self):
        return apex_workers_pb2_grpc.ApexWorkersStub(self.channel_sync)
    
    def get_apex_workers_stub_async(self):
        return apex_workers_pb2_grpc.ApexWorkersStub(self.channel_async)

    async def close(self, grace_period=1.0):
        await self.channel_async.close(grace_period)
        self.channel_sync.close()
