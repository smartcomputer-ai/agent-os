from __future__ import print_function
import asyncio
import grpc
import logging
from concurrent import futures
from aos.runtime.apex import apex_api_pb2, apex_api_pb2_grpc
from aos.runtime.apex import apex_workers_pb2, apex_workers_pb2_grpc
from aos.runtime.store.base_client import BaseClient

import logging
logger = logging.getLogger(__name__)

class ApexClient(BaseClient):
    def __init__(self, server_address="localhost:50052"):
        super().__init__(server_address)
    
    def get_apex_api_stub_sync(self):
        return apex_api_pb2_grpc.ApexApiStub(self.channel_sync)
    
    def get_apex_api_stub_async(self):
        return apex_api_pb2_grpc.ApexApiStub(self.channel_async)
    
    def get_apex_workers_stub_sync(self):
        return apex_workers_pb2_grpc.ApexWorkersStub(self.channel_sync)
    
    def get_apex_workers_stub_async(self):
        return apex_workers_pb2_grpc.ApexWorkersStub(self.channel_async)
