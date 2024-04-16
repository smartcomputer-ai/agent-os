import asyncio
from concurrent import futures
import logging
import grpc
from grpc import Server
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc
from .lmdb_backend import LmdbBackend

class GritStore(grit_store_pb2_grpc.GritStoreServicer):

    def __init__(self, backend:LmdbBackend) -> None:
        super().__init__()
        self._backend = backend

    def Store(
            self, 
            request: grit_store_pb2.StoreRequest, 
            context: grpc.aio.ServicerContext,
            ) -> grit_store_pb2.StoreResponse:
        return self._backend.store(request)

    def Load(
            self, 
            request: grit_store_pb2.LoadRequest, 
            context: grpc.aio.ServicerContext,
            ) -> grit_store_pb2.LoadResponse:
        return self._backend.load(request)


async def start_server(grit_dir:str, port:str="50051") -> Server:
    lmdb_backend = LmdbBackend(grit_dir, writemap=True)
    #kind of a hack to switch from asyc to sync lmdb handling (which is mostly sync)
    server = grpc.aio.server(futures.ThreadPoolExecutor(max_workers=10))
    grit_store_pb2_grpc.add_GritStoreServicer_to_server(GritStore(lmdb_backend), server)
    server.add_insecure_port("[::]:" + port)
    await server.start()
    print("Server started, listening on " + port)
    return server


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    async def arun():
        server = await start_server("/tmp/grit_store")
        await server.wait_for_termination()
    asyncio.run(arun())