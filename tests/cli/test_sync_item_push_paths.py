import os
import pytest
from aos.grit import *
from aos.wit import *
import aos.cli.sync_item as sync_item
import helpers_sync as helpers

async def test_sync_from_push_path__with_files(tmp_path):
    #files and dirs are sorted alphabetically
    tmp_path = os.path.relpath(tmp_path)
    helpers.create_file(tmp_path, "file1.txt", "content1")
    helpers.create_file(tmp_path, "file2.html", "<html>Congrats!</html>")
    helpers.create_file(tmp_path, "file3.json", {"name": "John", "age": 30, "city": "New York"})
    helpers.create_file(f"{tmp_path}/code", "wit1.py", "python contents")
    helpers.create_file(f"{tmp_path}/code", "wit2.py", "python contents")

    pushpath = f"{tmp_path}:/"
    sync_items = sync_item.sync_from_push_path(pushpath)
    assert len(sync_items) == 5
    assert sync_items[0].dir_path == str(tmp_path)
    assert sync_items[0].core_path == "/"
    assert sync_items[0].file_name == "file1.txt"
    assert sync_items[0].item_name == "file1.txt"

    assert sync_items[3].dir_path == os.path.join(tmp_path, "code")
    assert sync_items[3].core_path == "/code"
    assert sync_items[3].file_name == "wit1.py"
    assert sync_items[3].item_name == "wit1.py"

async def test_sync_from_push_path__empty(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    pushpath = f"{tmp_path}:/"
    sync_items = sync_item.sync_from_push_path(pushpath)
    assert len(sync_items) == 0

    pushpath = f"{tmp_path}:"
    sync_items = sync_item.sync_from_push_path(pushpath)
    assert len(sync_items) == 0

    pushpath = f"{tmp_path}"
    sync_items = sync_item.sync_from_push_path(pushpath)
    assert len(sync_items) == 0

async def test_sync_from_push_path__not_existing(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    with pytest.raises(ValueError):
        pushpath = f"{tmp_path}/notexist:/"
        sync_items = sync_item.sync_from_push_path(pushpath)

async def test_sync_from_push_path__invalid_core_path(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    with pytest.raises(ValueError):
        pushpath = f"{tmp_path}/notexist:blah"
        sync_items = sync_item.sync_from_push_path(pushpath)

async def test_sync_from_push_path__with_ignore(tmp_path):
    tmp_path = os.path.relpath(tmp_path)
    helpers.create_file(tmp_path, "file1.txt", "content1")
    helpers.create_file(os.path.join(tmp_path, "__pycache__"), "cache", "content1")
    helpers.create_file(os.path.join(tmp_path, ".grit"), "cache", "content1")
    
    pushpath = f"{tmp_path}:/"
    sync_items = sync_item.sync_from_push_path(pushpath, ignore=["/__pycache__", ".grit"])
    assert len(sync_items) == 1
    assert sync_items[0].dir_path == str(tmp_path)



