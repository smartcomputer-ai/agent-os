from grit.object_model import *
from grit.object_serialization import *
from grit.object_store import ObjectStore

class MemoryObjectStore(ObjectStore):
    #no lockign needed here, because all the dict operations used here are atomic
    _store:dict[ObjectId, Object]
    
    def __init__(self):
        super().__init__()
        self._store = {}

    async def store(self, object:Object) -> ObjectId:
        return self.store_sync(object)
    
    async def load(self, object_id:ObjectId) -> Object | None:
        return self.load_sync(object_id)
    
    def store_sync(self, object:Object) -> ObjectId:
        bytes = object_to_bytes(object)
        object_id = get_object_id(bytes)
        self._store[object_id] = bytes
        return object_id
    
    def load_sync(self, object_id:ObjectId) -> Object | None:
        bytes = self._store.get(object_id)
        if bytes is None:
            return None
        return bytes_to_object(bytes)
    