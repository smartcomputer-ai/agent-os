from aos.grit.object_model import *
from aos.grit.object_serialization import *
from aos.grit.object_store import ObjectStore
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc
from .store_client import StoreClient

#todo: this should probably move to the worker? depending who needs it

#todo: add a cache to the store, track the total bytes stored (using the object size), set limits there

class AgentObjectStore(ObjectStore):
    """An object store for a single agent. It connects to the Grit store server to perist data."""
    def __init__(self, store_client:StoreClient, agent_id:ActorId):
        super().__init__()
        self._agent_id = agent_id
        self._store_client = store_client
        self._store_stub_sync = store_client.get_grit_store_stub_sync()
        self._store_stub_async = store_client.get_grit_store_stub_async()

    def _to_store_request(self, object:Object):
        data = object_to_bytes(object)
        object_id = get_object_id(data)
        return grit_store_pb2.StoreRequest(
            agent_id=self._agent_id, 
            object_id=object_id, 
            data=data)
    
    def _to_load_request(self, object_id:ObjectId):
        return grit_store_pb2.LoadRequest(
            agent_id=self._agent_id, 
            object_id=object_id)

    async def store(self, object:Object) -> ObjectId:
        request = self._to_store_request(object)
        await self._store_stub_async.Store(request)
        return request.object_id
    
    async def load(self, object_id:ObjectId) -> Object | None:
        response:grit_store_pb2.LoadResponse = await self._store_stub_async.Load(
            self._to_load_request(object_id))
        if response.data is None:
            return None
        return bytes_to_object(response.data)
    
    def store_sync(self, object:Object) -> ObjectId:
        request = self._to_store_request(object)
        self._store_stub_sync.Store(request)
        return request.object_id
    
    def load_sync(self, object_id:ObjectId) -> Object | None:
        response:grit_store_pb2.LoadResponse = self._store_stub_sync.Load(
            self._to_load_request(object_id))
        if response.data is None:
            return None
        return bytes_to_object(response.data)