from __future__ import print_function
import asyncio
import grpc
import logging
from concurrent import futures

class BaseClient:
    def __init__(self, server_address):
        self.server_address = server_address
        # the async and sync api cannot be shared
        # however, opening two channels is okay, because, appratenly, there is something called a "sun channel"
        # wich is a shared resource between the two client channels (if their configuration is the same)
        # see: https://stackoverflow.com/a/62761510 (last comment)
        self.channel_async = grpc.aio.insecure_channel(self.server_address)
        self.channel_sync = grpc.insecure_channel(self.server_address)

    async def wait_for_async_channel_ready(self):
        await self.channel_async.channel_ready()
        state = self.channel_async.get_state(try_to_connect=True)
        print(f"Channel state: {state}")


    def get_channel_sync(self):
        return self.channel_sync
    
    def get_channel_async(self):
        return self.channel_async
    
    async def close(self, grace_period=1.0):
        await self.channel_async.close(grace_period)
        self.channel_sync.close()

    @classmethod
    async def get_connected_client_with_retry(cls, 
            server_address,
            max_retries=5, 
            retry_interval_seconds=5.0,
            logger:logging.Logger=None):
        client_name = cls.__name__        
        if logger:
            logger.info(f"{client_name}: Connecting to server '{server_address}'...")
        tries = 0
        while True:
            tries += 1
            try:
                print("connect 1")
                client = cls(server_address)
                print("connect 2")
                await client.wait_for_async_channel_ready()
                print("connect 3")
                if logger:
                    logger.info(f"{client_name}: Connected server '{server_address}' after {tries} try(ies).")
                return client
            except grpc.aio.AioRpcError as e:
                if tries >= max_retries:
                    if logger:
                        logger.error(f"{client_name}: Max retries reached trying to connect to server, giving up.")
                    raise e
                else:
                    if logger:
                        logger.warning(f"{client_name}: Was not able to connect to server '{server_address}', will try again in {retry_interval_seconds} seconds, {(max_retries-tries)} tries left. gRPC code: {str(e.code())}")
                    await asyncio.sleep(retry_interval_seconds)
