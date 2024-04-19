import os
import asyncio
from concurrent import futures
from typing import AsyncIterable
import grpc
from grpc import Server
from aos.runtime.apex import apex_api_pb2, apex_api_pb2_grpc
from aos.runtime.apex import apex_workers_pb2, apex_workers_pb2_grpc
from .apex_core_loop import ApexCoreLoop

import logging
logger = logging.getLogger(__name__)


class ApexApi(apex_api_pb2_grpc.ApexApiServicer):
    def __init__(self, core_loop:ApexCoreLoop):
        self.core_loop = core_loop

    async def GetApexStatus(self, request: apex_api_pb2.GetApexStatusRequest, context):
        state = await self.core_loop.get_state_copy()
        return apex_api_pb2.GetApexStatusResponse(
            state=apex_api_pb2.GetApexStatusResponse.UNKNOWN, #TODO: implement
            node_id=self.core_loop._node_id,
            store_address=self.core_loop._store_address,
            workers=[agent.to_apex_api_worker_info() for agent in state.workers.values()]
        )
    
    async def GetRunningAgents(self, request: apex_api_pb2.GetRunningAgentsRequest, context) -> apex_api_pb2.GetRunningAgentsResponse:
        state = await self.core_loop.get_state_copy()
        return apex_api_pb2.GetRunningAgentsResponse(
            agents=[agent.to_apex_api_agent_info() for agent in state.agents.values()]
        )

    async def StartAgent(self, request: apex_api_pb2.StartAgentRequest, context):
        await self.core_loop.start_agent(request.agent_id)
        return apex_api_pb2.StartAgentResponse()

    async def StopAgent(self, request: apex_api_pb2.StopAgentRequest, context):
        await self.core_loop.stop_agent(request.agent_id)
        return apex_api_pb2.StopAgentResponse()

    async def InjectMessage(self, request: apex_api_pb2.InjectMessageRequest, context):
        raise NotImplementedError()


class ApexWorkers(apex_workers_pb2_grpc.ApexWorkersServicer):
    def __init__(self, core_loop:ApexCoreLoop):
        self.core_loop = core_loop

    async def RegisterWorker(self, request: apex_workers_pb2.WorkerRegistrationRequest, context) -> apex_workers_pb2.WorkerRegistrationResponse:
        ticket = await self.core_loop.register_worker(request.worker_id)
        return apex_workers_pb2.WorkerRegistrationResponse(ticket=ticket)

    async def ConnectWorker(
            self, 
            request_iterator: AsyncIterable[apex_workers_pb2.WorkerToApexMessage], 
            context,
            ) -> AsyncIterable[apex_workers_pb2.ApexToWorkerMessage]:

        logger.info("ApexWorkers.ConnectWorker: Starting worker stream")

        #the queue through which the worker loop communicates back with the client (because this is a two-way gRPC stream)
        to_worker_queue:asyncio.Queue[apex_workers_pb2.ApexToWorkerMessage] = asyncio.Queue()

        async def process_incoming_messages():
            connected = False
            worker_id = None

            try:
                async for message in request_iterator:
                    if message.type == apex_workers_pb2.WorkerToApexMessage.PING:
                        logger.info(f"ApexWorkers.ConnectWorker: Worker {message.worker_id} sent PING.")
                    #only accept READY messags once
                    elif message.type == apex_workers_pb2.WorkerToApexMessage.READY and not connected:
                        logger.info(f"ApexWorkers.ConnectWorker: Worker {message.worker_id} sent READY with ticket {message.ticket}.")
                        connected = True
                        worker_id = message.worker_id
                        await self.core_loop.worker_connected(message.worker_id, message.ticket, message.manifest, to_worker_queue)
                    elif not connected:
                        logger.warning(f"ApexWorkers.ConnectWorker: Worker {message.worker_id} sent message ({message.type}) before READY.")
                        return
                    elif message.type == apex_workers_pb2.WorkerToApexMessage.RETURN_AGENT:
                        #todo: implement
                        logger.warning(f"ApexWorkers.ConnectWorker: Worker {message.worker_id} sent RETURN_AGENT with ticket {message.ticket}. NOT IMPLEMENTED YET.")
            except Exception as e:
                logger.error(f"ApexWorkers.ConnectWorker: Error in incoming message processing: {e}")
            finally:
                #when the loop ends, the worker has disconnected
                if connected and worker_id:
                    logger.info(f"ApexWorkers.ConnectWorker: Worker {worker_id} disconnected.")
                    #only forward the disconnect to the core loop if the worker was connected (had sent READY)
                    await self.core_loop.worker_disconnected(worker_id)
                else:
                    logger.warning("ApexWorkers.ConnectWorker: Worker disconnected without ever sending READY.")
                    #in the other case, the core loop closes the worker_queue
                    to_worker_queue.put_nowait(None)

        #start the incoming message processing task
        process_incoming_task = asyncio.create_task(process_incoming_messages())

        #start the response to worker loop
        #note: when the incoming tasks completes, the server seems to stop the entire ConnectWorker request function call and the rest of the function is not called
        #      but we are sending the disconnect from within the task, that should work
        try:
            while True:
                message:apex_workers_pb2.ApexToWorkerMessage|None = await to_worker_queue.get()
                if message is None:
                    #the loop has terminated the connection
                    logger.info("ApexWorkers.ConnectWorker: Outgoing queue terminated.")
                    break
                else:
                    logger.info(f"ApexWorkers.ConnectWorker: Sending message to worker: {message.type}")
                    yield message
        except Exception as e:
            logger.error(f"ApexWorkers.ConnectWorker: Error in outgoing message processing: {e}")
        finally:
            #cancel the incoming message processing task
            logger.info("ApexWorkers.ConnectWorker: Cancelling incoming message process.")
            process_incoming_task.cancel()
            logger.info("ApexWorkers.ConnectWorker: Two-way stream terminated.")


async def start_server(
        port:str="50052", 
        store_address:str="localhost:50051",
        node_id:str|None=None,
        assign_time_delay_secods:float=0,
        ) -> Server:
    
    #start core loop
    core_loop = ApexCoreLoop(store_address, node_id, assign_time_delay_secods)
    core_loop_task = asyncio.create_task(core_loop.start())
    await core_loop.wait_until_running()

    #start server
    server = grpc.aio.server(futures.ThreadPoolExecutor(max_workers=5)) #not many workers needed, as the server is entirely async
    apex_api_pb2_grpc.add_ApexApiServicer_to_server(ApexApi(core_loop), server)
    apex_workers_pb2_grpc.add_ApexWorkersServicer_to_server(ApexWorkers(core_loop), server)
    server.add_insecure_port("[::]:" + port)
    await server.start()
    server_task = asyncio.create_task(server.wait_for_termination())
    print("Server started, listening on " + port)
    await asyncio.wait([core_loop_task, server_task], return_when=asyncio.FIRST_COMPLETED)
    core_loop.stop()
    await asyncio.wait_for(core_loop_task, 0.5)
    await server.stop(0.5)
    await server_task
    print("Server stopped.")


# how to do graceful shutdown 

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    async def arun():
        await start_server()
    asyncio.run(arun())