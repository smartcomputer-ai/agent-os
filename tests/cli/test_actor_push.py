import os
import pytest
from aos.grit import *
from aos.wit import *
from aos.cli import ActorPush
import helpers_sync as helpers

def create_files(root_path):
    #files and dirs are sorted alphabetically
    helpers.create_file(root_path, "file1.txt", "content1")
    helpers.create_file(root_path, "file2.html", "<html>Congrats!</html>")
    helpers.create_file(root_path, "file3.json", {"name": "John", "age": 30, "city": "New York"})
    helpers.create_file(f"{root_path}/code", "wit1.py", "python contents")
    helpers.create_file(f"{root_path}/code", "wit2.py", "python contents")

async def test_actor_push__add_path(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    create_files(tmp_path)

    pushpath = f"{tmp_path}:/"
    push = ActorPush(is_genesis=False, actor_id=helpers.get_random_actor_id())
    push.add_push_path(pushpath)
    assert len(push.sync_items) == 5

async def test_actor_push__add_path_twice(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    create_files(f"{tmp_path}/a")
    create_files(f"{tmp_path}/b")
    
    push = ActorPush(is_genesis=False, actor_id=helpers.get_random_actor_id())
    push.add_push_path(f"{tmp_path}/a:/same-target")
    push.add_push_path(f"{tmp_path}/b:/same-target")
    #the paths should be merged, since they both are the same, the same # of files need to result
    assert len(push.sync_items) == 5

async def test_actor_push__to_core(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    create_files(tmp_path)

    pushpath = f"{tmp_path}:/"
    push = ActorPush(is_genesis=False, actor_id=helpers.get_random_actor_id())
    push.add_push_path(pushpath)
    core = push.to_core()

    assert core.has_key("file1.txt")
    file_blob = await core.getb("file1.txt")
    assert file_blob.get_headers()["ct"] == "s"
    assert file_blob.get_as_str() == "content1"

    assert core.has_key("file1.txt")
    file_blob = await core.getb("file2.html")
    assert file_blob.get_headers()["Content-Type"] == "text/html"
    assert file_blob.get_as_str() == "<html>Congrats!</html>"

    assert core.has_key("file3.json")
    file_blob = await core.getb("file3.json")
    assert file_blob.get_headers()["ct"] == "j"
    assert file_blob.get_as_json() == {"name": "John", "age": 30, "city": "New York"}

    file_blob = await core.get_path("code/wit1.py")
    assert file_blob.get_headers()["ct"] == "s"
    assert file_blob.get_as_str() == "python contents"
    
async def test_actor_push__to_core_prototype(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    create_files(tmp_path)

    pushpath = f"{tmp_path}:/"
    push = ActorPush(is_genesis=False, actor_id=helpers.get_random_actor_id())
    push.is_prototype = True
    push.add_push_path(pushpath)
    core = push.to_core()

    assert core.has_key("prototype")
    assert core.has_key("wit")
    assert core.has_key("wit_update")

    #check the prototype core
    prototype_core = await core.gett("prototype")
    assert prototype_core.has_key("file1.txt")
    file_blob = await prototype_core.getb("file1.txt")
    assert file_blob.get_headers()["ct"] == "s"
    assert file_blob.get_as_str() == "content1"

    assert prototype_core.has_key("file1.txt")
    file_blob = await prototype_core.getb("file2.html")
    assert file_blob.get_headers()["Content-Type"] == "text/html"
    assert file_blob.get_as_str() == "<html>Congrats!</html>"

    assert prototype_core.has_key("file3.json")
    file_blob = await prototype_core.getb("file3.json")
    assert file_blob.get_headers()["ct"] == "j"
    assert file_blob.get_as_json() == {"name": "John", "age": 30, "city": "New York"}

    file_blob = await prototype_core.get_path("code/wit1.py")
    assert file_blob.get_headers()["ct"] == "s"
    assert file_blob.get_as_str() == "python contents"
    


