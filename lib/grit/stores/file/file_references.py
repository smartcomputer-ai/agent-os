import asyncio
import os
from pathlib import PureWindowsPath
import threading
from grit.object_model import *
from grit.references import References

class FileReferences(References):
    # to share the dictionary between sync and async code, a retrant lock is needed
    # the event loop, when executing coroutines, can re-enter, but other threads can't
    _thread_lock:threading.RLock
    # however, to coordinate the async coroutines, also a async lock is needed
    _async_lock:asyncio.Lock

    _ref:dict[str, ObjectId]

    def __init__(self, store_path:str):
        super().__init__()
        self._async_lock = asyncio.Lock()
        self._thread_lock = threading.RLock()
        self._ref = {}
        self.store_path = store_path
        self.references_path = os.path.join(store_path, 'refs')
        os.makedirs(os.path.join(store_path, 'refs'), exist_ok=True)
        #walk the refs directory and load all the references
        for root, _dirs, files in os.walk(self.references_path):
            for file in files:
                ref = os.path.relpath(os.path.join(root, file), self.references_path)
                if os.name == "nt": #convert the path to forward slash posix path
                    ref = PureWindowsPath(ref).as_posix()
                with open(os.path.join(root, file), "r") as f:
                    object_id = bytes.fromhex(f.read())
                    self._ref[ref] = object_id


    async def get(self, ref:str) -> ObjectId | None:
        return self.get_sync(ref)
    
    async def get_all(self) -> dict[str, ObjectId]:
        return self.get_all_sync()

    async def set(self, ref:str, object_id:ObjectId):
        with self._thread_lock:
            async with self._async_lock:
                self._set_and_persist(ref, object_id)

    def get_sync(self, ref:str) -> ObjectId | None:
        return self._ref.get(ref, None)

    def get_all_sync(self) -> dict[str, ObjectId]:
        return self._ref.copy()

    def set_sync(self, ref:str, object_id:ObjectId) -> None:
        with self._thread_lock:
            self._set_and_persist(ref, object_id)

    def _set_and_persist(self, ref:str, object_id:ObjectId) -> None:
        self._ref[ref] = object_id
        #save to file
        file_path = os.path.join(self.references_path, ref)
        dir_path = os.path.dirname(file_path)
        if not os.path.exists(dir_path):
            os.makedirs(dir_path, exist_ok=True)
        with open(file_path, 'w') as f:
            f.write(object_id.hex())