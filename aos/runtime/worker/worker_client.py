from __future__ import print_function
import asyncio
import grpc
import logging
from concurrent import futures
from aos.runtime.worker import worker_api_pb2, worker_api_pb2_grpc
from aos.runtime.store.base_client import BaseClient

import logging
logger = logging.getLogger(__name__)

class WorkerClient(BaseClient):
    def __init__(self, server_address="localhost:50053"):
        super().__init__(server_address)
    
    def get_worker_api_stub_sync(self):
        return worker_api_pb2_grpc.WorkerApiStub(self.channel_sync)
    
    def get_worker_api_stub_async(self):
        return worker_api_pb2_grpc.WorkerApiStub(self.channel_async)
    