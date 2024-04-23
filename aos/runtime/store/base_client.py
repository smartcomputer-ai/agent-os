from __future__ import print_function
import asyncio
import grpc
import logging
import time
from concurrent import futures

class BaseClient:
    def __init__(self, server_address):
        self.server_address = server_address
        # the async and sync api cannot be shared
        # however, opening two channels is okay, because, appratenly, there is something called a "sun channel"
        # wich is a shared resource between the two client channels (if their configuration is the same)
        # see: https://stackoverflow.com/a/62761510 (last comment)
        self.channel_async = grpc.aio.insecure_channel(
            self.server_address,
            options=[
                #("grpc.enable_retries", 0),
                # ("grpc.min_reconnect_backoff_ms", 5000),
                # ("grpc.max_reconnect_backoff_ms", 10000),
                #('grpc.enable_http_proxy', 0),
            ])
        
        self.channel_sync = grpc.insecure_channel(self.server_address)

    async def wait_for_async_channel_ready(self, timeout_seconds:float=3000):
        try:
            await asyncio.wait_for(self.channel_async.channel_ready(), timeout_seconds)
        except asyncio.TimeoutError as e:
            raise asyncio.TimeoutError(f"{type(self).__name__}: Timeout waiting for {timeout_seconds} seconds for channel to be ready.") from e

    def get_channel_sync(self):
        return self.channel_sync
    
    def get_channel_async(self):
        return self.channel_async
    
    async def close(self, grace_period=1.0):
        await self.channel_async.close(grace_period)
        self.channel_sync.close()
