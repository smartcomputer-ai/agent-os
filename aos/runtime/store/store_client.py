from __future__ import print_function
import asyncio
import grpc
from concurrent import futures
from aos.runtime.store import grit_store_pb2_grpc, agent_store_pb2_grpc, agent_store_pb2
from .base_client import BaseClient

import logging
logger = logging.getLogger(__name__)

# the idea is that only one of these clients exists and then the object store and refs classes create one off stubs

class StoreClient(BaseClient):
    def __init__(self, server_address="localhost:50051"):
        super().__init__(server_address)

    def get_grit_store_stub_sync(self):
        return grit_store_pb2_grpc.GritStoreStub(self.channel_sync)
    
    def get_grit_store_stub_async(self):
        return grit_store_pb2_grpc.GritStoreStub(self.channel_async)
    
    def get_agent_store_stub_sync(self):
        return agent_store_pb2_grpc.AgentStoreStub(self.channel_sync)
    
    def get_agent_store_stub_async(self):
        return agent_store_pb2_grpc.AgentStoreStub(self.channel_async)
