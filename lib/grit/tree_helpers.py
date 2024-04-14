from typing import AsyncIterator, Iterable
from . object_model import *
from . object_store import *
from . object_serialization import *

# Low-level helpers for working with Grit trees.

# Note: for most functions here there is an async and a sync version.
# this results in a lot of code duplication (colored functions and all).
# To maintainer: whenever you make a change, also change the sync version 
# and vice versa.
#
# All these helpers are read-only. It would be too difficult to offer 
# low-level, path-based write operations. Instead the BlobObject and TreeObject
# classes in the wit package should be used for that.

#============================================================
# Internal Path Helpers
#============================================================
def _tree_path_parts(path:str, allow_absolute:bool=False) -> list[str]:
    """Returns a list of path parts, with empty parts removed"""
    if(path == "" or path is None):
        return []
    path = path.replace("//", "/")
    if(path[0] == "/"):
        if not allow_absolute:
            raise ValueError(f"Path must be relative, but was '{path}'.")
        path = path[1:]
    if(path[-1] == "/"):
        path = path[:-1]
    parts = path.split("/")
    parts = [part for part in parts if part != ""]
    return parts

def _blob_path_parts(path:str, allow_absolute:bool=False) -> list[str]:
    if(path == ""):
        raise ValueError(f"Blob path cannot be empty, but was '{path}'.")
    path = path.replace("//", "/")
    if(path[0] == "/"):
        if not allow_absolute:
            raise ValueError(f"Path must be relative, but was '{path}'.")
        path = path[1:]
    if(path[-1] == "/"):
        raise ValueError(f"Path to blob item must not end with a slash, but was '{path}'.")
    parts = path.split("/")
    parts = [part for part in parts if part != ""]
    return parts

#============================================================
# Load Path Helpers
#============================================================
async def _load_path(
    loader:ObjectLoader, 
    root_id:TreeId, 
    path_parts:list[str],
    possible_file_endings:list[str]=None
    ) -> tuple[TreeId|None, ObjectId, Tree|Blob]:
    """Returns the parent tree_id, the object_id, and the object at the end of the path"""
    tree_id = root_id
    tree:Tree = await loader.load(tree_id)
    if not is_tree(tree):
        raise ValueError(f"root tree '{root_id.hex()}' does not point to a tree.")
    if(path_parts is None or len(path_parts) == 0):
        return None, tree_id, tree
    for name in path_parts[:-1]:
        if name not in tree:
            raise ValueError(f"root tree '{root_id.hex()}' path '{path_parts}' does not contain '{name}'.")
        tree_id = tree[name]
        tree = await loader.load(tree_id)
        if tree is None:
            raise ValueError(f"root tree '{root_id.hex()}' path '{path_parts}' could not find '{name}' and id '{tree_id.hex()}'.")
        if not is_tree(tree):
            raise ValueError(f"root tree '{root_id.hex()}' path '{path_parts}' found '{name}' with id '{tree_id.hex()}', but it was not a sub-tree.")
    final_fragment = path_parts[-1]
    if(final_fragment not in tree and possible_file_endings is not None and len(possible_file_endings) > 0):
        for ending in possible_file_endings:
            ending = ending.lstrip(".")
            final_fragment = f"{final_fragment}.{ending}"
            if final_fragment in tree:
                break
    if final_fragment not in tree:
        raise ValueError(f"root tree '{root_id.hex()}' path '{path_parts}' could not find '{path_parts[-1]}' "+
                         f"(nor the same ending in '{possible_file_endings}').")
    obj_id = tree[final_fragment]
    obj = await loader.load(obj_id)
    return tree_id, obj_id, obj

def _load_path_sync(
    loader:ObjectLoader, 
    root_id:TreeId, 
    path_parts:list[str],
    possible_file_endings:list[str]=None
    ) -> tuple[TreeId|None, ObjectId, Tree|Blob]:
    """Returns the parent tree_id, the object_id, and the object at the end of the path"""
    tree_id = root_id
    tree:Tree = loader.load_sync(tree_id)
    if not is_tree(tree):
        raise ValueError(f"root tree '{tree_id.hex()}' does not point to a tree.")
    if(path_parts is None or len(path_parts) == 0):
        return None, tree_id, tree
    for name in path_parts[:-1]:
        if name not in tree:
            raise ValueError(f"root tree '{root_id.hex()}' path '{path_parts}' does not contain '{name}'.")
        tree_id = tree[name]
        tree = loader.load_sync(tree_id)
        if tree is None:
            raise ValueError(f"root tree '{root_id.hex()}' path '{path_parts}' could not find '{name}' and id '{tree_id.hex()}'.")
        if not is_tree(tree):
            raise ValueError(f"root tree '{root_id.hex()}' path '{path_parts}' found '{name}' with id '{tree_id.hex()}', but it was not a sub-tree.")
    final_fragment = path_parts[-1]
    if(final_fragment not in tree and possible_file_endings is not None and len(possible_file_endings) > 0):
        for ending in possible_file_endings:
            ending = ending.lstrip(".")
            final_fragment = f"{final_fragment}.{ending}"
            if final_fragment in tree:
                break
    if final_fragment not in tree:
        #print("final fragemnt", final_fragment, tree)
        raise ValueError(f"root tree '{root_id.hex()}' path '{path_parts}' could not find '{path_parts[-1]}' "+
                         f"(nor the same ending in '{possible_file_endings}').")
    obj_id = tree[final_fragment]
    obj = loader.load_sync(obj_id)
    return tree_id, obj_id, obj

async def load_path(loader:ObjectLoader, root_id:TreeId, path:str) -> Tree|Blob:
    """Returns the object at the end of the path"""
    path_parts = _tree_path_parts(path)
    _, _, obj = await _load_path(loader, root_id, path_parts)
    return obj

async def load_tree_path(loader:ObjectLoader, root_id:TreeId, path:str) -> Tree:
    """Returns the tree object at the end of the path"""
    obj = await load_path(loader, root_id, path)
    if not is_tree(obj):
        raise ValueError(f"root tree '{root_id.hex()}' found '{path}' but it was not a sub-tree.")
    return obj

async def load_blob_path(loader:ObjectLoader, root_id:TreeId, path:str) -> Blob:
    """Returns the blob object at the end of the path"""
    path_parts = _blob_path_parts(path)
    _, _, obj = await _load_path(loader, root_id, path_parts)
    if not is_blob(obj):
        raise ValueError(f"root tree '{root_id.hex()}' found '{path}' but it was not a sub-blob.")
    return obj

def load_path_sync(loader:ObjectLoader, root_id:TreeId, path:str) -> Tree|Blob:
    """Returns the object at the end of the path"""
    path_parts = _tree_path_parts(path)
    _, _, obj = _load_path_sync(loader, root_id, path_parts)
    return obj

def load_tree_path_sync(loader:ObjectLoader, root_id:TreeId, path:str) -> Tree:
    """Returns the tree object at the end of the path"""
    obj = load_path_sync(loader, root_id, path)
    if not is_tree(obj):
        raise ValueError(f"root tree '{root_id.hex()}' found '{path}' but it was not a sub-tree.")
    return obj

def load_blob_path_sync(loader:ObjectLoader, root_id:TreeId, path:str) -> Blob:
    """Returns the blob object at the end of the path"""
    path_parts = _blob_path_parts(path)
    _, _, obj = _load_path_sync(loader, root_id, path_parts)
    if not is_blob(obj):
        raise ValueError(f"root tree '{root_id.hex()}' found '{path}' but it was not a sub-blob.")
    return obj

#============================================================
# Walk Helpers
#============================================================
async def walk_ids(loader:ObjectLoader, root_id:TreeId) -> AsyncIterator[tuple[TreeId, dict[str, TreeId], dict[str, BlobId]]]:
    """Asynchonously yields a tuple of (tree_id, trees, blobs) for each tree in the tree hierarchy. Similar to os.walk.
    
    Tree Id is the id of the current tree.
    Trees is a dictionary of [key:tree_id] of sub trees. Which can be edited while iterating to avoid descending into certain trees.
    Blobs is a dictionary of [key:blob_id] of sub blobs.
    """
    root_tree:Tree = await loader.load(root_id)
    if not is_tree(root_tree):
        raise ValueError(f"root tree '{root_id.hex()}' does not point to a tree.")
    trees = {}
    blobs = {}
    for key, object_id in root_tree:
        value = await loader.load(object_id)
        if(is_tree(value)):
            trees[key] = object_id
        else:
            blobs[key] = object_id
    yield (root_id, trees, blobs)
    for _, tree_id in trees.items():
        async for sub_tree in walk_ids(loader, tree_id):
            yield sub_tree

def walk_ids_sync(loader:ObjectLoader, root_id:TreeId) -> Iterable[tuple[TreeId, dict[str, TreeId], dict[str, BlobId]]]:
    """Yields a tuple of (tree_id, trees, blobs) for each tree in the tree hierarchy. Similar to os.walk.
    
    Tree Id is the id of the current tree.
    Trees is a dictionary of [key:tree_id] of sub trees. Which can be edited while iterating to avoid descending into certain trees.
    Blobs is a dictionary of [key:blob_id] of sub blobs.
    """
    root_tree:Tree = loader.load_sync(root_id)
    if not is_tree(root_tree):
        raise ValueError(f"root tree '{root_id.hex()}' does not point to a tree.")
    trees = {}
    blobs = {}
    for key, object_id in root_tree:
        value = loader.load_sync(object_id)
        if(is_tree(value)):
            trees[key] = object_id
        else:
            blobs[key] = object_id
    yield (root_id, trees, blobs)
    for _, tree_id in trees.items():
        for sub_tree in walk_ids_sync(loader, tree_id):
            yield sub_tree
