import logging
import os
import lmdb
from aos.grit import *
from aos.runtime.store import grit_store_pb2

_GRIT_ID_LEN = 32

logger = logging.getLogger(__name__)

class LmdbBackend:
    def __init__(self, store_path:str, writemap:bool=False):
        self.store_path = store_path
        self._resizing = False
        os.makedirs(self.store_path, exist_ok=True)
        self.env = lmdb.Environment(
            store_path, 
            max_dbs=5, 
            # writemap=True is what makes lmdb FAST (about 10x faster than if its False), 
            # BUT it makes the DB file as big as the mapsize (at least on some file systems). 
            # Plus, it comes with fewer safety guarantees.
            # See: https://lmdb.readthedocs.io/en/release/#writemap-mode
            writemap=writemap, 
            metasync=False, 
            # Flush write buffers asynchronously to disk
            # if wirtemap is False, this is ignored
            map_async=True,
            # 10 MB, is ignored if it's bigger already
            map_size=1024*1024*10, 
            )

    #=========================================================
    # Env API
    # Docs: https://lmdb.readthedocs.io/en/release/#environment-class
    #=========================================================
    def get_env(self) -> lmdb.Environment:
        return self.env
    
    def get_object_db(self) -> lmdb._Database:
        return self.env.open_db('obj'.encode('utf-8'))
    
    def get_refs_db(self) -> lmdb._Database:
        return self.env.open_db('refs'.encode('utf-8'))
    
    def begin_object_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_object_db(), write=write, buffers=buffers)

    def begin_refs_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_refs_db(), write=write, buffers=buffers)
    

    def _resize(self) -> int:
        #TODO: is this safe? Do we need to lock in some way or other?
        # probably not, because it is only run in a single process
        self._resizing = True
        current_size = self.env.info()['map_size']
        if current_size > 1024*1024*1024*10: # 10 GB
            multiplier = 1.2
        elif current_size > 1024*1024*1024: # 1 GB
            multiplier = 1.5
        else: # under 1 GB
            multiplier = 3.0
        # must be rounded to next int! otherwise lmdb will segfault later (spent several hours on this)
        new_size = round(current_size * multiplier) 
        logger.info(f"Resizing LMDB map from {current_size/1024/1024} MB to {new_size/1024/1024} MB")
        self.env.set_mapsize(new_size)
        self._resizing = False
        return new_size
    

    #=========================================================
    # Grit Object Store API
    #=========================================================
    def store(self, request:grit_store_pb2.StoreRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        
        #check the object id
        object_id = get_object_id(request.data)
        if request.object_id is not None and not is_object_id_match(object_id, request.object_id):
            raise ValueError(f"object_id in request does not match object_id derived from request data.")
        object_key = _make_object_key(request.agent_id, object_id)
        try:
            with self.begin_object_txn() as txn:
                txn.put(object_key, request.data, overwrite=False)
        except lmdb.MapFullError:
            logger.warn(f"===> Resizing LMDB map... in obj store, (obj id: {object_id.hex()}) <===")
            self._resize()
            #try again
            with self.begin_object_txn() as txn:
                txn.put(object_key, request.data, overwrite=False)

        return None
    

    def load(self, request:grit_store_pb2.LoadRequest) -> grit_store_pb2.LoadResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        if(not is_object_id(request.object_id)):
            raise TypeError(f"object_id must be of type ObjectId, not '{type(request.object_id)}'.")
        object_key = _make_object_key(request.agent_id, request.object_id)
        with self.begin_object_txn(write=False) as txn:
            bytes = txn.get(object_key, default=None)
        
        return grit_store_pb2.LoadResponse(
            agent_id=request.agent_id,
            object_id=request.object_id,
            data=bytes)
    

    #=========================================================
    # References Store API
    #=========================================================
    def set_ref(self, request:grit_store_pb2.SetRefRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        ref_key = _make_refs_key(request.agent_id, request.ref)
        with self.begin_refs_txn() as txn:
            txn.put(ref_key, request.object_id)

        return None


    def get_ref(self, request:grit_store_pb2.GetRefRequest)  -> grit_store_pb2.GetRefResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        ref_key = _make_refs_key(request.agent_id, request.ref)
        with self.begin_refs_txn(write=False) as txn:
            object_id = txn.get(ref_key, default=None)
        
        return grit_store_pb2.GetRefResponse(
            agent_id=request.agent_id,
            ref=request.ref,
            object_id=object_id)
    

    def get_refs(self, request:grit_store_pb2.GetRefsRequest) -> grit_store_pb2.GetRefsResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        search_key = request.agent_id
        if request.ref_prefix is not None:
            search_key = _make_refs_key(request.agent_id, request.ref_prefix)
        refs = {}
        with self.begin_refs_txn(write=False) as txn:
            cursor = txn.cursor()
            cursor.set_range(search_key)
            while _bytes_startswith(cursor.key(), search_key):
                _, ref = _parse_refs_key(cursor.key())
                refs[ref] = cursor.value()
                cursor.next()

        return grit_store_pb2.GetRefsResponse(
            agent_id=request.agent_id,
            refs=refs)

        
#=========================================================
# Utils
#=========================================================
def _make_object_key(agent_id:ActorId, object_id:ObjectId) -> bytes:
    return agent_id + object_id

def _parse_object_key(key:bytes) -> tuple[ActorId, ObjectId]:
    return key[:_GRIT_ID_LEN], key[_GRIT_ID_LEN:]

def _make_refs_key(agent_id:ActorId, ref:str) -> bytes:
    return agent_id + ref.encode('utf-8')

def _parse_refs_key(key:bytes) -> tuple[ActorId, str]:
    return key[:_GRIT_ID_LEN], key[_GRIT_ID_LEN:].decode('utf-8')

def _bytes_startswith(haystack:bytes, needle:bytes) -> bool:
    return haystack[:len(needle)] == needle