import os
import asyncio
from concurrent import futures
import logging
from typing import AsyncIterable
import grpc
from grpc import Server
from aos.runtime.apex import apex_api_pb2, apex_api_pb2_grpc
from aos.runtime.apex import apex_workers_pb2, apex_workers_pb2_grpc

class ApexApi(apex_api_pb2_grpc.ApexApiServicer):

    async def GetRunningAgents(self, request: apex_api_pb2.GetRunningAgentsRequest, context):
        pass

    async def StartAgent(self, request: apex_api_pb2.StartAgentRequest, context):
        pass

    async def StopAgent(self, request: apex_api_pb2.StopAgentRequest, context):
        pass

    async def InjectMessage(self, request: apex_api_pb2.InjectMessageRequest, context):
        pass

    async def GetApexStatus(self, request: apex_api_pb2.GetApexStatusRequest, context):
        pass


class ApexWorkers(apex_workers_pb2_grpc.ApexWorkersServicer):

    async def RegisterWorker(self, request: apex_workers_pb2.WorkerRegistrationRequest, context):
        pass

    async def ConnectWorker(
            self, 
            request_iterator: AsyncIterable[apex_workers_pb2.WorkerToApexMessage], 
            context,
            ) -> AsyncIterable[apex_workers_pb2.ApexToWorkerMessage]:

        async def process_incoming_messages():
            async for message in request_iterator:
                print("received message from worker", message.worker_id, message.type)
            print("process incoming stream done")

        #note: when the incoming tasks completes, the server seems to stop the entire request and the rest of the function is not called
        process_incoming_task = asyncio.create_task(process_incoming_messages())
        await asyncio.sleep(10)
        print("canceling worker stream")
        process_incoming_task.cancel()
        
        print("worker stream done")


#example: https://github.com/grpc/grpc/blob/master/examples/python/route_guide/asyncio_route_guide_server.py
    # async def RouteChat(
    #     self,
    #     request_iterator: AsyncIterable[route_guide_pb2.RouteNote],
    #     unused_context,
    # ) -> AsyncIterable[route_guide_pb2.RouteNote]:
    #     prev_notes = []
    #     async for new_note in request_iterator:
    #         for prev_note in prev_notes:
    #             if prev_note.location == new_note.location:
    #                 yield prev_note
    #         prev_notes.append(new_note)



async def start_server(port:str="50052") -> Server:
    #kind of a hack to switch from asyc to sync lmdb handling (which is mostly sync)
    server = grpc.aio.server(futures.ThreadPoolExecutor(max_workers=10))
    apex_api_pb2_grpc.add_ApexApiServicer_to_server(ApexApi(), server)
    apex_workers_pb2_grpc.add_ApexWorkersServicer_to_server(ApexWorkers(), server)
    server.add_insecure_port("[::]:" + port)
    await server.start()
    print("Server started, listening on " + port)
    return server

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    async def arun():
        server = await start_server()
        await server.wait_for_termination()
    asyncio.run(arun())