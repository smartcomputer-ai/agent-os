import os
import pytest
from pathlib import PureWindowsPath, PurePosixPath
from aos.grit.stores.memory import MemoryReferences, MemoryObjectStore
from aos.grit import *
from aos.wit import *
import aos.cli.sync_file as sync_file
import helpers_sync as helpers

def create_files(root_path):
    helpers.create_file(f"{root_path}/code/common", "common_code1.py", "python common contents")
    helpers.create_file(f"{root_path}/code/common", "some_lib.py", "python common contents")

    helpers.create_file(f"{root_path}/code/wit_a", "some_module.py", "python module contents")
    helpers.create_file(f"{root_path}/code/wit_a", "wit.py", "python wit function contents")
    helpers.create_file(f"{root_path}/code/wit_a", "query.py", "python wit query function contents")

    helpers.create_file(f"{root_path}/code/wit_b", "some_module.py", "python module contents")
    helpers.create_file(f"{root_path}/code/wit_b", "another_wit.py", "python wit function contents")

    helpers.create_file(f"{root_path}/data/common", "file1.txt", "content1")
    helpers.create_file(f"{root_path}/data/common", "file2.html", "<html>Congrats!</html>")
    helpers.create_file(f"{root_path}/data_a", "file.json", {"name": "John", "age": 30, "city": "New York"})
    helpers.create_file(f"{root_path}/data_a_gen", "genesis_file.json", {"name": "Dude", "age": 1, "city": "Crystal Lake"})
    helpers.create_file(f"{root_path}/data_a_sync", "sync_file.txt", "hi")
    helpers.create_file(f"{root_path}/data_b", "file.json", {"name": "Luke", "age": 21, "city": "Buffalo"})

    if os.name == "nt" and "\\" in root_path:
        posix_root_path = PureWindowsPath(root_path).as_posix()
    else:
        posix_root_path = root_path

    toml_string=f'''
[all] 
push = ["{posix_root_path}/code/common:/code/common", "{posix_root_path}/data/common:/data/common"]

[[actors]]
name = "a"
push = ["{posix_root_path}/code/wit_a:/code/", "{posix_root_path}/data_a:/data"]
push_value."/data/args" = "hello world"
push_value."/data/more_args" = {{"hello" = "world"}}
push_on_genesis = "{posix_root_path}/data_a_gen:/data"
pull = "path/to/common_code:/"
sync = "{posix_root_path}/data_a_sync/sync_file.txt:/data/sync_file.txt" #supports both push and pull
wit = "/code:wit:wit_a" 
wit_query = "/code:query:wit_query_a" 
#wit_update = "/code:module:function_name" 

[[actors]]
name = "b"
push = ["{posix_root_path}/code/wit_b:/code/", "{posix_root_path}/data_b:/data"]
wit = "external:wit_b:wit_b" 
wit_query = "external:wit_b:qit_b_query" 
runtime = "python" #which runtime to use, default is python
'''
    helpers.create_file(f"{root_path}", "sync.toml", toml_string)

async def test_sync_file_from_toml_file(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    create_files(tmp_path)
    refs = MemoryReferences()
    pushes = await sync_file.load_pushes(f"{tmp_path}/sync.toml", refs)
    assert len(pushes) == 2

    assert pushes[0].actor_name == "a"
    for item in pushes[0].sync_items:
        print(item)
    assert len(pushes[0].sync_items) == 12 # 10 files, and 2 valus

    assert pushes[1].actor_name == "b"
    assert len(pushes[1].sync_items) == 7 # 7 files

async def test_sync_file_push(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    create_files(tmp_path)
    store = MemoryObjectStore()
    refs = MemoryReferences()
    pushes = await sync_file.load_pushes(f"{tmp_path}/sync.toml", refs)
    assert len(pushes) == 2

    for push in pushes:
        await push.create_and_inject_messages(store, refs, 1)

    all_refs = await refs.get_all()
    for ref, id in all_refs.items():
        print(ref, id.hex())
    assert len(all_refs) == 4 # actor, step head, and 2 agent refs

async def test_sync_file_push_twice(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    create_files(tmp_path)
    store = MemoryObjectStore()
    refs = MemoryReferences()
    pushes = await sync_file.load_pushes(f"{tmp_path}/sync.toml", refs)
    for push in pushes:
        await push.create_and_inject_messages(store, refs, 1)

    #must clear out the previous genesis message and replace it with the new one
    pushes = await sync_file.load_pushes(f"{tmp_path}/sync.toml", refs)
    for push in pushes:
        await push.create_and_inject_messages(store, refs, 1)

    all_refs = await refs.get_all()
    for ref, id in all_refs.items():
        print(ref, id.hex())
    assert len(all_refs) == 4 # actor, step head, and 2 agent refs