from aos.grit.object_model import *
from aos.grit.object_serialization import *
from aos.grit.references import References
from aos.runtime.store import grit_store_pb2
from .grit_store_client import GritStoreClient

#todo: this should probably move to the worker? depending who needs it

#todo: add a cache to the store, track the total bytes stored (using the object size), set limits there

class AgentReferences(References):
    """An references store for a single agent. It connects to the Grit store server to perist the references."""
    def __init__(self, store_client:GritStoreClient, agent_id:ActorId):
        super().__init__()
        self._agent_id = agent_id
        self._store_client = store_client
        self._store_stub_sync = store_client.get_store_stub_sync()
        self._store_stub_async = store_client.get_store_stub_async()


    async def get(self, ref:str) -> ObjectId | None:
        request = grit_store_pb2.GetRefRequest(agent_id=self._agent_id, ref=ref)
        response:grit_store_pb2.GetRefResponse = await self._store_stub_async.GetRef(request)
        if response.object_id is None:
            return None
        return response.object_id


    async def get_all(self) -> dict[str, ObjectId]:
        request = grit_store_pb2.GetRefsRequest(agent_id=self._agent_id)
        response:grit_store_pb2.GetRefsResponse = await self._store_stub_async.GetRefs(request)
        return {ref: object_id for ref, object_id in response.refs.items()}


    async def set(self, ref:str, object_id:ObjectId) -> None:
        request = grit_store_pb2.SetRefRequest(agent_id=self._agent_id, ref=ref, object_id=object_id)
        await self._store_stub_async.SetRef(request)


    def get_sync(self, ref:str) -> ObjectId | None:
        request = grit_store_pb2.GetRefRequest(agent_id=self._agent_id, ref=ref)
        response:grit_store_pb2.GetRefResponse = self._store_stub_sync.GetRef(request)
        if response.object_id is None:
            return None
        return response.object_id


    def get_all_sync(self) -> dict[str, ObjectId]:
        request = grit_store_pb2.GetRefsRequest(agent_id=self._agent_id)
        response:grit_store_pb2.GetRefsResponse = self._store_stub_sync.GetRefs(request)
        return {ref: object_id for ref, object_id in response.refs.items()}


    def set_sync(self, ref:str, object_id:ObjectId) -> None:
        request = grit_store_pb2.SetRefRequest(agent_id=self._agent_id, ref=ref, object_id=object_id)
        self._store_stub_sync.SetRef(request)
