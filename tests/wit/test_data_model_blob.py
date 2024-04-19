import os
from aos.grit import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit.data_model import *

async def test_blob__get_set_persist__bytes():
    store = MemoryObjectStore()
    blob = BlobObject(None, None)
    test_data = bytes(b'hello world')
    blob.set_as_bytes(test_data)
    test_data_2 = blob.get_as_bytes()
    assert test_data == test_data_2

    object_id = await blob.persist(store)
    blob_2 = await store.load(object_id)
    assert blob_2 is not None
    assert blob_2.data == test_data

async def test_blob__get_set_persist__str():
    store = MemoryObjectStore()
    blob = BlobObject(None, None)
    test_data = 'hello world'
    blob.set_as_str(test_data)
    test_data_2 = blob.get_as_str()
    assert test_data == test_data_2

    object_id = await blob.persist(store)
    blob_2 = await store.load(object_id)
    assert blob_2 is not None
    assert blob_2.data == bytes(test_data, 'utf-8')

async def test_blob__get_set_persist__dict():
    store = MemoryObjectStore()
    blob = BlobObject(None, None)
    test_data = {
        'hello': 'world',
        'but': [1,2,3,5],
        'also': {"this": "that"}
        }
    blob.set_as_json(test_data)
    test_data_2 = blob.get_as_json()
    assert test_data == test_data_2

    object_id = await blob.persist(store)
    test_data_3 = await store.load(object_id)
    assert test_data_3 is not None