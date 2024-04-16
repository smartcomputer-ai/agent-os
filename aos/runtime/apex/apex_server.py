import os
import asyncio
from concurrent import futures
import logging
import grpc
from grpc import Server
from aos.runtime.apex import apex_pb2, apex_pb2_grpc


class Apex(apex_pb2_grpc.ApexServicer):

    async def RegisterWorker(self, request: apex_pb2.WorkerRegistrationRequest, context):
        pass

    async def WorkerStream(self, request: apex_pb2.WorkerToApex, context):
        pass


async def start_server(port:str="50052") -> Server:
    #kind of a hack to switch from asyc to sync lmdb handling (which is mostly sync)
    server = grpc.aio.server(futures.ThreadPoolExecutor(max_workers=10))
    apex_pb2_grpc.add_ApexServicer_to_server(Apex(), server)
    server.add_insecure_port("[::]:" + port)
    await server.start()
    print("Server started, listening on " + port)
    return server