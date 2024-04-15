import logging
import os
import lmdb

logger = logging.getLogger(__name__)

class SharedEnvironment:
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