# run the grit server

import asyncio
import logging

import grpc
from aos.cluster.protos import helloworld_pb2, helloworld_pb2_grpc

class Greeter(helloworld_pb2_grpc.GreeterServicer):
    async def SayHello(
        self,
        request: helloworld_pb2.HelloRequest,
        context: grpc.aio.ServicerContext,
    ) -> helloworld_pb2.HelloReply:
        logging.info("Received hello world from: %s",  request.name)
        return helloworld_pb2.HelloReply(message="Hello, %s!" % request.name)
    
async def runboth() -> None:
    server = grpc.aio.server()
    helloworld_pb2_grpc.add_GreeterServicer_to_server(Greeter(), server)
    listen_addr = "[::]:50051"
    server.add_insecure_port(listen_addr)
    logging.info("Starting server on %s", listen_addr)
    await server.start()
    
    asyncio.sleep(0.1)
    logging.info("Starting client")

    async with grpc.aio.insecure_channel("localhost:50051") as channel:
        stub = helloworld_pb2_grpc.GreeterStub(channel)
        response = await stub.SayHello(helloworld_pb2.HelloRequest(name="you"))
    print("Greeter client received: " + response.message)
    

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    asyncio.run(runboth())
