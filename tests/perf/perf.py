import os
import sys
import time
import asyncio
import tempfile
from grit.stores.lmdb import SharedEnvironment, LmdbReferences, LmdbObjectStore
from . perf_grid import perf_grid_run

async def amain():
    # store = MemoryObjectStore()
    # refs = MemoryReferences()
    with tempfile.TemporaryDirectory() as temp_dir:
        print(f"Temp dir is {temp_dir}")
        # store = FileObjectStore(temp_dir)
        # refs = FileReferences(temp_dir)
        shared_env = SharedEnvironment(temp_dir, writemap=True)
        store = LmdbObjectStore(shared_env)
        refs = LmdbReferences(shared_env)
        # store = MemoryObjectStore()
        # refs = MemoryReferences()
        await perf_grid_run(store, refs)
        file_path = temp_dir
        files = os.listdir(file_path)
        file_bytes = sum(os.path.getsize(os.path.join(file_path,f)) for f in files) / 1024 / 1024
        print(f"Temp dir {file_path} has {len(files)} files, and is {file_bytes:0.2f} MB")

def main():
    t = time.perf_counter()
    asyncio.run(amain())
    t2 = time.perf_counter()

    print(f'Total time elapsed: {t2-t:0.2f} seconds')
    sys.exit(0)

if __name__ == "__main__":
    main()