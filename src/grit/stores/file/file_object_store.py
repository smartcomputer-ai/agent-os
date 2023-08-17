import os
import asyncio
import threading
import aiofiles
from functools import lru_cache
from async_lru import alru_cache
from grit.object_model import *
from grit.object_serialization import *
from grit.object_store import ObjectStore

class FileObjectStore(ObjectStore):

    # to share the dictionary between sync and async code, a retrant lock is needed
    # the event loop, when executing coroutines, can re-enter, but other threads can't
    _thread_lock:threading.RLock
    # however, to coordinate the async coroutines, also a async lock is needed
    _async_lock:asyncio.Lock

    def __init__(self, store_path:str):
        super().__init__()
        self._thread_lock = threading.RLock()
        self._async_lock = asyncio.Lock()
        self.store_path = store_path
        self.object_path = os.path.join(store_path, 'obj')
        #ensure that the paths exists
        os.makedirs(self.object_path, exist_ok=True)

    async def store(self, object:Object) -> ObjectId:
        bytes, object_id, object_path = self._to_bytes_and_path(object)
        #check if the object already exists
        # this is safe to do outside the lock, because only one thread or coroutine is allowed to write any file at a time
        if os.path.exists(object_path):
            return object_id
        with self._thread_lock:
            async with self._async_lock:
                #if the file is more than 100KB, write it asynchronously
                # writing small files asynchronously makes the overall system much slower
                # however, the best cutoff of when to switch to async is not clear, I just picked it arbitrarily at 100KB
                if len(bytes) > 100000:
                    with open(object_path, 'wb') as f:
                        f.write(bytes)
                else:
                    async with aiofiles.open(object_path, 'wb') as f:
                        await f.write(bytes)
        return object_id
    
    def store_sync(self, object:Object) -> ObjectId:
        bytes, object_id, object_path = self._to_bytes_and_path(object)
        #check if the object already exists
        # this is safe to do outside the lock, because only one thread or coroutine is allowed to write any file at a time
        if os.path.exists(object_path):
            return object_id
        with self._thread_lock:
            with open(object_path, 'wb') as f:
                f.write(bytes)
        return object_id

    def _to_bytes_and_path(self, object:Object):
        bytes = object_to_bytes(object)
        object_id = get_object_id(bytes)
        object_id_str = object_id.hex()
        object_path = os.path.join(self.object_path, object_id_str)
        return bytes, object_id, object_path

    @alru_cache(maxsize=1024)
    async def load(self, object_id:ObjectId) -> Object | None:
        object_path = self._to_path(object_id)
        with self._thread_lock:
            async with self._async_lock:
                if not os.path.exists(object_path):
                    return None
                with open(object_path, 'rb') as f:
                    bytes = f.read()
        return bytes_to_object(bytes)
    
    @lru_cache(maxsize=1024)  # noqa: B019
    def load_sync(self, object_id:ObjectId) -> Object | None:
        object_path = self._to_path(object_id)
        with self._thread_lock:
            if not os.path.exists(object_path):
                return None
            with open(object_path, 'rb') as f:
                bytes = f.read()
        return bytes_to_object(bytes)
    
    def _to_path(self, object_id:ObjectId):
        object_id_str = object_id.hex()
        return os.path.join(self.object_path, object_id_str)
    

    