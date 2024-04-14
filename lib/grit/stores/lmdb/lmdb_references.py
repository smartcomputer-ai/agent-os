import logging
from grit.object_model import *
from grit.object_serialization import *
from grit.references import References
import lmdb
from .shared_env import SharedEnvironment

logger = logging.getLogger(__name__)

class LmdbReferences(References):
    def __init__(self, shared_env:SharedEnvironment):
        super().__init__()
        if(not isinstance(shared_env, SharedEnvironment)):
            raise Exception(f"shared_env must be of type SharedEnvironment, not '{type(shared_env)}'.")
        self._shared_env = shared_env

    async def set(self, ref:str, object_id:ObjectId) -> None:
        self.set_sync(ref, object_id)

    async def get(self, ref:str) -> ObjectId | None:
        return self.get_sync(ref)
    
    async def get_all(self) -> dict[str, ObjectId]:
        return self.get_all_sync()

    def set_sync(self, ref:str, object_id:ObjectId) -> None:
        if(ref is None):
            raise ValueError("ref must not be None.")
        ref_bytes = ref.encode('utf-8')

        try:
            with self._shared_env.begin_refs_txn() as txn:
                if not txn.put(ref_bytes, object_id, overwrite=True):
                    raise Exception(f"Not able to set '{ref}' in lmdb 'refs' database.")
        except lmdb.MapFullError as lmdb_error:
            logger.warn(f"===> Resizing LMDB map... in refs store, (ref: {ref}, obj id: {object_id.hex()}) <===")
            self._shared_env._resize()
            #try again
            with self._shared_env.begin_refs_txn() as txn:
                if not txn.put(ref_bytes, object_id, overwrite=True):
                    raise Exception(f"Not able to set '{ref}' in lmdb 'refs' database.") from lmdb_error


    def get_sync(self, ref:str) -> ObjectId | None:
        with self._shared_env.begin_refs_txn(write=False) as txn:
            return txn.get(ref.encode('utf-8'), default=None)

    def get_all_sync(self) -> dict[str, ObjectId]:
        with self._shared_env.begin_refs_txn(write=False) as txn:
            kv = dict(txn.cursor().iternext())
        return {k.decode('utf-8'): v for k, v in kv.items()}
    