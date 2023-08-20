from grit import *
from wit import *

class StoreWrapper:
    """Create a wrapper around the object store that has a flat interface, easy to understand and explain to an LLM."""
    def __init__(self, store:ObjectStore):
        self.messages = []
        self.store = store

    async def load_bytes(self, id:str) -> bytes | None:
        blob = await self.store.load(to_object_id(id))
        return blob.data if blob else None
    
    async def load_str(self, id:str) -> str | None:
        blob = await self.store.load(to_object_id(id))
        if blob is None:
            return None
        blob_obj = BlobObject(blob)
        return blob_obj.get_as_str()
    
    async def load_json(self, id:str) -> dict | None:
        blob = await self.store.load(to_object_id(id))
        if blob is None:
            return None
        blob_obj = BlobObject(blob)
        return blob_obj.get_as_json()
    
    async def store_bytes(self, data:bytes) -> str:
        obj_id = await BlobObject.from_bytes(data).persist(self.store)
        return obj_id.hex()
    
    async def store_str(self, data:str) -> str:
        obj_id = await BlobObject.from_str(data).persist(self.store)
        return obj_id.hex()
    
    async def store_json(self, data:dict) -> str:
        obj_id = await BlobObject.from_json(data).persist(self.store)
        return obj_id.hex()
