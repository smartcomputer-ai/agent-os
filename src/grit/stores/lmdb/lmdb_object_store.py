import logging
from functools import lru_cache
from async_lru import alru_cache
from grit.object_model import *
from grit.object_serialization import *
from grit.object_store import ObjectStore
import lmdb
from . shared_env import SharedEnvironment

logger = logging.getLogger(__name__)

class LmdbObjectStore(ObjectStore):
    def __init__(self, shared_env:SharedEnvironment):
        super().__init__()
        if(not isinstance(shared_env, SharedEnvironment)):
            raise Exception(f"shared_env must be of type SharedEnvironment, not '{type(shared_env)}'.")
        self._shared_env = shared_env

    async def store(self, object:Object) -> ObjectId:
        return self.store_sync(object)
    
    async def load(self, object_id:ObjectId) -> Object | None:
        return self.load_sync(object_id)
    
    def store_sync(self, object:Object) -> ObjectId:
        if(object is None):
            raise ValueError("object must not be None.")
        bytes = object_to_bytes(object)
        object_id = get_object_id(bytes)
        try:
            with self._shared_env.begin_object_txn() as txn:
                txn.put(object_id, bytes, overwrite=False)
            return object_id
        except lmdb.MapFullError:
            logger.warn(f"===> Resizing LMDB map... in obj store, (obj id: {object_id.hex()}) <===")
            self._shared_env._resize()
            #try again
            with self._shared_env.begin_object_txn() as txn:
                txn.put(object_id, bytes, overwrite=False)
            return object_id
    
    @lru_cache(maxsize=1024*10)  # noqa: B019
    def load_sync(self, object_id:ObjectId) -> Object | None:
        if(object_id is None):
            raise ValueError("object_id must not be None.")
        if(not is_object_id(object_id)):
            raise TypeError(f"object_id must be of type ObjectId, not '{type(object_id)}'.")
        with self._shared_env.begin_object_txn(write=False) as txn:
            bytes = txn.get(object_id, default=None)
        if bytes is None:
            return None
        return bytes_to_object(bytes)
    