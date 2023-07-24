import os

import pytest
from src.grit import *
from src.grit.stores.memory import MemoryObjectStore, MemoryReferences
from src.wit.data_model import *

async def test_tree__get_set_persist():
    store = MemoryObjectStore()
    tree = TreeObject(store, None, None)
    test_data = 'hello world'
    tree.maket('a').maket('b').makeb('data.txt').set_as_str(test_data)
    tree.maket('x').maket('y').makeb('data.txt') #no data set, should not persist
    object_id = await tree.persist(store)

    tree2 = await TreeObject.from_tree_id(store, object_id)
    test_data_3 = (await (await (await tree2.get('a')).get('b')).get('data.txt')).get_as_str()
    assert test_data == test_data_3
    assert (await tree2.get('x')) is None

    for tree_key in tree2:
        assert tree_key == "a"

async def test_tree__maket_twice():
    store = MemoryObjectStore()
    core = Core(store, {}, None)
    code = core.maket("code")
    code.maket("helperlib").makeb("helper.py").set_as_str("a")
    code.maket("helperlib").makeb("__init__.py").set_as_str("b")
    code.makeb("main.py").set_as_str("c")
    core_id = await core.persist()

    core2 = await Core.from_core_id(store, core_id)
    code2 = await core2.get("code")
    assert code2 is not None
    assert (await code2.get("helperlib")) is not None
    assert len((await code2.get("helperlib")).keys()) == 2


async def test_tree__make_tree_path():
    paths = ['a/b/c', 'a/b/c/', 'a//b/c/']
    for path in paths:
        print(path)
        tree = TreeObject(None, None, None)
        node = tree.maket_path(path)
        assert node.breadcrumb == 'c'
        assert node.parent is not None
        node = node.parent
        assert node.breadcrumb == 'b'
        assert node.parent is not None
        node = node.parent
        assert node.breadcrumb == 'a'
        assert node.parent is not None
        node = node.parent
        assert node.breadcrumb is None
        assert node.parent is None

async def test_tree__make_tree_path_same_node():
    tree = TreeObject(None, None, None)
    node = tree.maket_path('')
    assert node == tree

async def test_tree__make_tree_path__error_if_absolute():
    with pytest.raises(ValueError) as e_info:
        tree = TreeObject(None, None, None)
        node = tree.maket_path('/a/b/c')

    with pytest.raises(ValueError) as e_info:
        tree = TreeObject(None, None, None)
        node = tree.maket_path('/')

async def test_tree__make_blob_path():
    tree = TreeObject(None, None, None)
    node = tree.makeb_path('a/b/c')
    assert node.breadcrumb == 'c'
    assert isinstance(node, BlobObject)
    assert node.parent is not None
    node = node.parent
    assert node.breadcrumb == 'b'
    assert isinstance(node, TreeObject)
    assert node.parent is not None
    node = node.parent
    assert node.breadcrumb == 'a'
    assert node.parent is not None
    node = node.parent
    assert node.breadcrumb is None
    assert node.parent is None

async def test_tree__make_blob_path__error_if_absolute_or_slash_end():
    with pytest.raises(ValueError) as e_info:
        tree = TreeObject(None, None, None)
        node = tree.makeb_path('/a/b/c')

    with pytest.raises(ValueError) as e_info:
        tree = TreeObject(None, None, None)
        node = tree.makeb_path('a/b/c/')
