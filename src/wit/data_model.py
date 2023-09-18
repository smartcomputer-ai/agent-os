from __future__ import annotations
import logging
import posixpath as path
import asyncio
import json
from typing import AsyncIterator, Callable, Iterable, Type
from pydantic import BaseModel
from grit import *
from grit.tree_helpers import _tree_path_parts, _blob_path_parts

logger = logging.getLogger(__name__)

# Contains the most important high-level abstractions for working with Grit objects.
# Most Grit objects from '../grit/object_model.py' have equivalent classes here.

#===================================================================================================
# BlobObject, TreeObject, and Core
# These form the main utility objects to work with grit data.
#===================================================================================================
class BlobObject:
    """Wraps a Grit blob object which is just a binary array with headers.

    Provides methods to work with common data types like bytes, str, json, and pydantic models,
    both loading and saving them as a blob.
    """
    STR_ENCODING:str = 'utf-8'
    __headers:dict[str, str]
    __data: bytes | None
    __blob_id: BlobId | None
    __parent:TreeObject | None
    __breadcrumb:str
    __dirty:bool

    def __init__(
            self, 
            blob:Blob|None, 
            blob_id:BlobId|None=None, 
            parent:TreeObject|None=None, 
            breadcrumb:str=None,
            ):
        if(blob is not None and not is_blob(blob)):
            raise TypeError("blob must be a Blob")
        if(blob_id is not None and not is_object_id(blob_id)):
            raise TypeError("blob_id must be a BlobId")
        self.__headers = blob.headers if blob and blob.headers else {}
        self.__data = blob.data if blob and blob.data else None
        self.__blob_id = blob_id #needed to bypass saving the blob if not dirty
        self.__parent = parent
        self.__breadcrumb = breadcrumb
        self.__dirty = False if blob_id else True #if an existing blob is loaded, assume it's not dirty

    @classmethod
    async def from_blob_id(cls, loader:ObjectLoader, blob_id:BlobId, parent:TreeObject=None):
        blob = await loader.load(blob_id)
        return cls(blob, blob_id, parent)

    @classmethod
    def from_blob(cls, blob:Blob):
        if(not is_blob(blob)):
            raise TypeError("blob must be a Blob")
        return cls(blob, None)

    @classmethod
    def from_bytes(cls, bytes:bytes):
        obj = cls(None, None)
        obj.set_as_bytes(bytes)
        return obj

    @classmethod
    def from_str(cls, s:str):
        obj = cls(None, None)
        obj.set_as_str(s)
        return obj
    
    @classmethod
    def from_json(cls, json:dict|BaseModel):
        obj = cls(None, None)
        obj.set_as_json(json)
        return obj
    
    @classmethod
    def from_content(cls, content:bytes|str|dict):
        if isinstance(content, bytes):
            return cls.from_bytes(content)
        elif isinstance(content, str):
            return cls.from_str(content)
        elif isinstance(content, dict):
            return cls.from_json(content)
        else:
            raise TypeError("content must be bytes, str, or dict") 

    @property
    def blob_id(self) -> BlobId|None:
        return self.__blob_id
    @property
    def parent(self) -> TreeObject|None:
        return self.__parent
    @property
    def breadcrumb(self) -> str|None:
        return self.__breadcrumb
    
    def mark_dirty(self):
        self.__dirty = True
        self.from_blob_id = None
        if(self.__parent is not None):
            self.__parent.mark_dirty()

    def get_as_bytes(self) -> bytes:
        if(self.__data is None):
            return None
        return self.__data
    
    def get_as_str(self) -> str:
        if(self.__data is None):
            return None
        return self.__data.decode(self.STR_ENCODING)
    
    def get_as_json(self) -> dict:
        if(self.__data is None):
            return None
        return json.loads(self.__data.decode(self.STR_ENCODING))
    
    def get_as_blob(self) -> Blob:
        if(self.__data is None):
            return None
        return Blob(self.__headers.copy() if len(self.__headers) > 0 else None, self.__data)

    #TODO make return type generic
    def get_as_model(self, pydantic_type:Type[BaseModel]) -> BaseModel:
        if(not issubclass(pydantic_type, BaseModel)):
            raise TypeError("pydantic_type must be a subclass of pydantic.BaseModel")
        if(self.__data is None):
            return None
        return pydantic_type(**self.get_as_json())

    def get_as_object_id(self) -> BlobId:
        if(self.__dirty):
            raise Exception("Cannot get blob id, blob is dirty")
        if(self.__blob_id is None):
            raise Exception("Cannot get blob id, blob has not been persisted yet")
        return self.__blob_id

    def set_as_bytes(self, data:bytes) -> BlobObject:
        if(data is None):
            raise Exception("Cannot set blob to None")
        if(not isinstance(data, bytes)):
            raise Exception("blob must be bytes")
        self.__data = data
        self.__headers['ct'] = 'b'
        self.mark_dirty()
        return self

    def set_as_str(self, s:str) -> BlobObject:
        if(s is None):
            raise Exception("Cannot set s to None")
        if(not isinstance(s, str)):
            raise Exception("s must be str")
        self.__data = s.encode(self.STR_ENCODING)
        self.__headers['ct'] = 's'
        self.mark_dirty()
        return self

    def set_as_json(self, obj:dict|BaseModel) -> BlobObject:
        if(obj is None):
            raise Exception("Cannot set obj to None")
        if(isinstance(obj, dict)):
            self.__data = json.dumps(obj).encode(self.STR_ENCODING)
        elif(isinstance(obj, BaseModel)):
            self.__data = obj.model_dump_json().encode(self.STR_ENCODING)
        else:
            raise Exception(f"obj must be dict or BaseModel but was '{type(obj)}'.")
        self.__headers['ct'] = 'j'
        self.mark_dirty()
        return self

    def set_from_blob(self, blob:BlobObject|Blob) -> BlobObject:
        if is_blob(blob):
            self.__data = blob.data
            self.__headers = blob.headers.copy() if blob.headers is not None else {}
        else:
            self.__data = blob.__data
            self.__headers = blob.__headers.copy()
        self.mark_dirty()
        return self

    def set_empty(self) -> BlobObject:
        self.__data = None
        self.__headers = {}
        self.mark_dirty()
        return self

    def set_header(self, key:str, value:str) -> BlobObject:
        self.__headers[key] = value
        self.mark_dirty()
        return self
    
    def get_header(self, key:str) -> str | None:
        return self.__headers.get(key, None)
    
    def get_headers(self) -> dict[str, str]:
        return self.__headers.copy()

    def set_headers_empty(self) -> BlobObject:
        self.__headers = {}
        self.mark_dirty()
        return self
    
    def integrate(self, parent:TreeObject|None, breadcrumb:str):
        '''Integrates a blob object into an existing tree object. Usually called by TreeObject.add(). 
        Fails if the blob already belongs to a different TreeObject.'''
        if(self.__parent is not None):
            raise Exception("Cannot integrate blob into tree, already has a parent.")
        self.__parent = parent
        self.__breadcrumb = breadcrumb

    def is_empty(self) -> bool:
        return self.__data is None
    
    async def persist(self, store:ObjectStore) -> ObjectId:
        if(not self.__dirty and self.__blob_id is not None):
            return self.__blob_id
        if(self.is_empty()):
            raise Exception("Cannot persist empty object")
        self.__blob_id = await store.store(Blob(self.__headers, self.__data))
        self.__dirty = False
        return self.__blob_id
    
    def path(self) -> str:
        if(self.__parent is None):
            tree_path = ""
        else:
            tree_path = self.__parent.path()
        if(self.__breadcrumb is not None):
            tree_path = path.join(tree_path, self.__breadcrumb)
        return path.normpath(tree_path)

    def __repr__(self) -> str:
        return f"BlobObject({self.path()})"

class TreeObject:
    """Work with a Grit tree efficiently.

    The class only loads data that is needed. 
    It provides several recursive methods that make it much easier to work with paths to sub-trees or sub-blobs.
    """
    __loader:ObjectLoader
    __tree:dict[str, ObjectId | TreeObject | BlobObject]
    __tree_id:TreeId|None
    __parent:TreeObject
    __breadcrumb:str
    __dirty:bool

    def __init__(
            self, 
            loader:ObjectLoader|None, 
            tree:Tree, 
            tree_id:TreeId|None=None, 
            tree_parent:TreeObject|None=None, 
            breadcrumb:str=None,
            ):
        self.__loader = loader
        if(tree is None):
            tree = {}
        self.__tree = tree.copy() #make a copy because the values will get replaced with Tree or Blob objects
        self.__tree_id = tree_id #needed to bypass saving the tree if not dirty
        self.__parent = tree_parent
        self.__breadcrumb = breadcrumb
        self.__dirty = False if tree_id else True #if an existing tree is loaded, assume it's not dirty

    @classmethod
    async def from_tree_id(cls, loader:ObjectLoader, tree_id:TreeId):
        tree = await loader.load(tree_id)
        return cls(loader, tree, tree_id)
   
    @property
    def tree_id(self) -> TreeId|None:
        return self.__tree_id
    @property
    def parent(self) -> TreeObject|None:
        return self.__parent
    @property
    def breadcrumb(self) -> str|None:
        return self.__breadcrumb

    def mark_dirty(self):
        self.__dirty = True
        self.__tree_id = None
        if(self.__parent is not None):
            self.__parent.mark_dirty()

    #==========================================
    # Get object APIs
    #==========================================
    def __get_set_object(self, key:str, obj_id:ObjectId, obj:Object):
        if(is_tree(obj)):
            sub_tree = TreeObject(self.__loader, obj, obj_id, self, key)
            self.__tree[key] = sub_tree
            return sub_tree
        else:
            sub_blob = BlobObject(obj, obj_id, self, key)
            self.__tree[key] = sub_blob
            return sub_blob
        
    async def get(self, key:str) -> TreeObject | BlobObject | None:
        if(key in self.__tree):
            value = self.__tree[key]
            #if the content hasn't been loaded yet, do it now, and replace the object id in the dict with the mod object
            if(isinstance(value, ObjectId)):
                if(self.__loader is None):
                    raise Exception("Cannot descend TreeObject that has no loader. Create an instance with the ObjectLoader set.")
                obj = await self.__loader.load(value)
                return self.__get_set_object(key, value, obj)
            else: # content was already loaded, so just return it
                return value
        else:
            return None
        
    def get_sync(self, key:str) -> TreeObject | BlobObject | None:
        if(key in self.__tree):
            value = self.__tree[key]
            #if the content hasn't been loaded yet, do it now, and replace the object id in the dict with the mod object
            if(isinstance(value, ObjectId)):
                if(self.__loader is None):
                    raise Exception("Cannot descend TreeObject that has no loader. Create an instance with the ObjectLoader set.")
                obj = self.__loader.load_sync(value)
                return self.__get_set_object(key, value, obj)
            else: # content was already loaded, so just return it
                return value
        else:
            return None

    async def __get_with_factory(self, key:str, factory:Callable[[str], TreeObject | BlobObject]) -> TreeObject | BlobObject:
        value = await self.get(key)
        if (value is not None):
            return value
        else:
            value = factory(key)
            self.__tree[key] = value
            self.mark_dirty()
            return value
        
    def __get_with_factory_sync(self, key:str, factory:Callable[[str], TreeObject | BlobObject]) -> TreeObject | BlobObject:
        value = self.get_sync(key)
        if (value is not None):
            return value
        else:
            value = factory(key)
            self.__tree[key] = value
            self.mark_dirty()
            return value
        
    async def gett(self, key:str) -> TreeObject:
        value = await self.__get_with_factory(key, lambda key: TreeObject(self.__loader, {}, None, self, key))
        if(not isinstance(value, TreeObject)):
            raise TypeError(f"'{key}' exists, but is not not a TreeObject, it is a '{type(value)}'.")
        return value

    async def getb(self, key:str) -> BlobObject:
        value = await self.__get_with_factory(key, lambda key: BlobObject(None, None, self, key))
        if(not isinstance(value, BlobObject)):
            raise TypeError(f"'{key}' exists, but is not not a BlobObject, it is a '{type(value)}'.")
        return value
    
    def gett_sync(self, key:str) -> TreeObject:
        value =  self.__get_with_factory_sync(key, lambda key: TreeObject(self.__loader, {}, None, self, key))
        if(not isinstance(value, TreeObject)):
            raise TypeError(f"'{key}' exists, but is not not a TreeObject, it is a '{type(value)}'.")
        return value

    def getb_sync(self, key:str) -> BlobObject:
        value = self.__get_with_factory_sync(key, lambda key: BlobObject(None, None, self, key))
        if(not isinstance(value, BlobObject)):
            raise TypeError(f"'{key}' exists, but is not not a BlobObject, it is a '{type(value)}'.")
        return value

    #==========================================
    # Make sub-object APIs
    #==========================================
    def maket(self, key:str, exist_ok:bool=True) -> TreeObject:
        if(not isinstance(key, str)):
            raise TypeError(f"key must be a string, not a '{type(key)}'.")
        if(key in self.__tree):
            if(exist_ok):
                return self.gett_sync(key)
            else:
                raise Exception(f"Key '{key}' already exists in tree. To use make with an existing key, set exist_ok=True.")
        value = TreeObject(self.__loader, {}, None, self, key)
        self.__tree[key] = value
        self.mark_dirty()
        return value
    
    def makeb(self, key:str|any, exist_ok:bool=True) -> BlobObject:
        if(not isinstance(key, str)):
            raise TypeError(f"key must be a string, not a '{type(key)}'.")
        if(key in self.__tree):
            if(exist_ok):
                return self.getb_sync(key)
            else:
                raise Exception(f"Key '{key}' already exists in tree. To use make with an existing key, set exist_ok=True.")
        value = BlobObject(None, None, self, key)
        self.__tree[key] = value
        self.mark_dirty()
        return value

    #==========================================
    # Get and make path to objects APIs
    #==========================================
    async def get_path(self, path:str) -> TreeObject | BlobObject | None:
        parts = _tree_path_parts(path)
        if(len(parts) == 1):
            return await self.get(parts[0])
        else:
            next = await self.get(parts[0])
            if(next is None):
                return None
            elif(isinstance(next, BlobObject)):
                raise ValueError(f"Cannot descend into blob '{parts[0]}' in path '{path}'. "+
                                 "Path fragments, except the last one, must be tree objects.")
            else:
                return await next.get_path("/".join(parts[1:]))
            
    def get_path_sync(self, path:str) -> TreeObject | BlobObject | None:
        parts = _tree_path_parts(path)
        if(len(parts) == 1):
            return self.get_sync(parts[0])
        else:
            next = self.get_sync(parts[0])
            if(next is None):
                return None
            elif(isinstance(next, BlobObject)):
                raise ValueError(f"Cannot descend into blob '{parts[0]}' in path '{path}'. "+
                                 "Path fragments, except the last one, must be tree objects.")
            else:
                return next.get_path_sync("/".join(parts[1:]))

    def maket_path(self, path:str, exist_ok:bool=True) -> TreeObject:
        parts = _tree_path_parts(path)
        if(len(parts) == 0):
            return self
        if(len(parts) == 1):
            return self.maket(parts[0], exist_ok)
        else:
            return self.maket(parts[0], exist_ok).maket_path("/".join(parts[1:]), exist_ok)
    
    def makeb_path(self, path:str, exist_ok:bool=True) -> BlobObject:
        parts = _blob_path_parts(path)
        if(len(parts) == 0):
            return self
        if(len(parts) == 1):
            return self.makeb(parts[0], exist_ok)
        else:
            return self.maket(parts[0], exist_ok).makeb_path("/".join(parts[1:]), exist_ok)
    
    async def walk(self) -> AsyncIterator[tuple[str, list[TreeObject], list[BlobObject]]]:
        trees = []
        blobs = []
        for key, value in self.__tree.items():
            if(isinstance(value, ObjectId)):
                value = await self.get(key)
            if(isinstance(value, TreeObject)):
                trees.append(value)
            elif(isinstance(value, BlobObject)):
                blobs.append(value)
        yield (self.path(), trees, blobs)
        for tree in trees:
            async for sub_tree in tree.walk():
                yield sub_tree

    def walk_sync(self) -> Iterable[tuple[str, list[TreeObject], list[BlobObject]]]:
        trees = []
        blobs = []
        for key, value in self.__tree.items():
            if(isinstance(value, ObjectId)):
                value = self.get_sync(key)
            if(isinstance(value, TreeObject)):
                trees.append(value)
            elif(isinstance(value, BlobObject)):
                blobs.append(value)
        yield (self.path(), trees, blobs)
        for tree in trees:
            for sub_tree in tree.walk_sync():
                yield sub_tree

    #==========================================
    # Add and integrate existing objecs APIs
    #==========================================
    def add(self, key:str, object:ObjectId|TreeObject|BlobObject):
        if(key in self.__tree):
            raise Exception(f"'{key}' already exists")
        #if it's a blob or tree object, try integrate first, because can fail
        if(isinstance(object, TreeObject) or isinstance(object, BlobObject)):
            object.integrate(self, key)
        self.__tree[key] = object
        self.mark_dirty()
    
    def integrate(self, parent:TreeObject|None, breadcrumb:str):
        '''Integrates a tree object into an existing tree object. Usually called by TreeObject.add(). 
        Fails if this sub-tree already belongs to a different TreeObject.'''
        if(self.__parent is not None):
            raise Exception("Cannot integrate blob into tree, already has a parent.")
        self.__parent = parent
        self.__breadcrumb = breadcrumb

    #==========================================
    # Persist entire tree, sub-trees, and sub-objects APIs
    #==========================================
    def is_empty(self) -> bool:
        if(len(self.__tree) == 0):
            return True
        #recursively check if the tree is empty
        # for a tree not to be empty, it must have at least one non-empty blob child or indirect child (e.g., of another sub-tree)
        for(_key, value) in self.__tree.items():
            if(isinstance(value, ObjectId)):
                return False #not empty because there is an object with and id, meaning it was successfully persisted
            elif(isinstance(value, TreeObject)):
                if(not value.is_empty()):
                    return False
            elif(isinstance(value, BlobObject)):
                if(not value.is_empty()):
                    return False
        return True
    
    async def persist(self, store:ObjectStore=None):
        if(not self.__dirty and self.__tree_id is not None):
            return self.__tree_id
        if(self.is_empty()):
            raise Exception("Cannot persist empty tree, at least one non-empty sub-object must be present")
        #if a store is provided and the tree has no loader, set it
        if(store is not None and self.__loader is None):
            self.__loader = store
        #if store is None, see if the __loader is an ObjectStore instance
        if(store is None):
            if(self.__loader is not None and isinstance(self.__loader, ObjectStore)):
                store = self.__loader
            else:
                raise Exception("Cannot persist tree, no ObjectStore instance provided, "+
                                "tried to see if the loader is an ObjectStore instance, but it is not.")
        tree_to_store = {}
        for(key, value) in self.__tree.items():
            if(isinstance(value, ObjectId)):
                tree_to_store[key] = value
            elif(isinstance(value, TreeObject)):
                #only persist non-empty trees
                if(not value.is_empty()):
                    tree_to_store[key] = await value.persist(store)
            elif(isinstance(value, BlobObject)):
                #only persist non-empty blobs
                if(not value.is_empty()):
                    tree_to_store[key] = await value.persist(store)
            else:
                raise TypeError(f"Cannot persist tree, invalid type for key '{key}'")
        self.__tree_id = await store.store(tree_to_store)
        self.__dirty = False
        return self.__tree_id

    #==========================================
    # Util APIs
    #==========================================
    def __len__(self):
        return len(self.__tree)

    def __delitem__(self, key):
        del self.__tree[key]
        self.mark_dirty()

    def path(self) -> str:
        if(self.__parent is None):
            tree_path = "/"
        else:
            tree_path = self.__parent.path()
        if(self.__breadcrumb is not None):
            tree_path = path.join(tree_path, self.__breadcrumb)
        return path.normpath(tree_path)

    def __repr__(self) -> str:
        return f"TreeObject({self.path()})"

    def __iter__(self):
        return iter(self.__tree)

    def clear(self):
        self.__tree.clear()
        self.mark_dirty()

    def has_key(self, k):
        return k in self.__tree
    
    def keys(self):
        return self.__tree.keys()
    
    def get_as_object_id(self) -> TreeId:
        if(self.__dirty):
            raise Exception("Cannot get tree id, tree is dirty")
        if(self.__tree_id is None):
            raise Exception("Cannot get tree id, tree has not been persisted yet")
        return self.__tree_id

    def get_as_tree(self) -> Tree:
        '''This only works if none of the tree objects are dirty. Otherwise, it will throw an exception.'''
        if(self.__dirty):
            raise Exception("Cannot get tree as grit dictionary, tree is dirty")
        tree = {}
        for(key, value) in self.__tree.items():
            if(isinstance(value, ObjectId)):
                tree[key] = value
            elif(isinstance(value, TreeObject)):
                tree[key] = value.get_as_object_id()
            elif(isinstance(value, BlobObject)):
                tree[key] = value.get_as_object_id()
            else:
                raise TypeError(f"Cannot get tree, invalid type for key '{key}'")
        return tree
    

class Core(TreeObject):
    def __init__(self, loader:ObjectLoader, tree:Tree, tree_id:TreeId|None):
        super().__init__(loader, tree, tree_id, tree_parent=None)
    #todo: add methods to ensure the shape of the core tree
   
    @classmethod
    async def from_core_id(cls, loader:ObjectLoader, core_id:TreeId) -> Core:
        core_tree = await loader.load(core_id)
        return cls(loader, core_tree, core_id)

    @classmethod
    def from_external_wit_ref(cls, loader:ObjectLoader, wit_ref:str, query_ref:str=None) -> Core:
        if(wit_ref is None):
            raise ValueError("wit_name cannot be None")
        core = cls(loader, {}, None)
        core.makeb('wit').set_as_str(f"external:{wit_ref}")
        if(query_ref is not None):
            core.makeb('wit_query').set_as_str(f"external:{query_ref}")
        return core
    
    def maket_path(self, path: str, exist_ok:bool=True) -> TreeObject:
        '''TreeObject only allows relative paths, since this is the root, absolute paths are allowed.'''
        if(len(path) > 0 and path[0] == '/'):
            path = path[1:]
        return super().maket_path(path, exist_ok)
    
    def makeb_path(self, path: str, exist_ok:bool=True) -> BlobObject:
        '''TreeObject only allows relative paths, since this is the root, absolute paths are allowed.'''
        if(len(path) > 0 and path[0] == '/'):
            path = path[1:]
        return super().makeb_path(path, exist_ok)
    
    async def get_path(self, path: str) -> TreeObject | BlobObject | None:
        '''TreeObject only allows relative paths, since this is the root, absolute paths are allowed.'''
        if(len(path) > 0 and path[0] == '/'):
            path = path[1:]
        return await super().get_path(path)
    
    async def merge(self, new_core:Core):
        '''Merges the new_core into the current core. The current core will be modified.'''
        # walk the new_core and see if the current core is different
        async for _path, _trees, blobs in new_core.walk():
            for blob in blobs:
                new_blob_path = blob.path()
                try:
                    target_blob:BlobObject = await self.get_path(new_blob_path)
                except Exception as ex:
                    #this usually happens if one is a path and the other is a blob or vice versa
                    logger.warn(f"Merge conflic while getting {new_blob_path}: {ex}")
                    continue
                if(target_blob is None):
                    self.makeb_path(new_blob_path).set_from_blob(blob)
                else:
                    #compare by object id
                    target_object_id = target_blob.get_as_object_id()
                    new_object_id = blob.get_as_object_id()
                    if(target_object_id != new_object_id):
                        target_blob.set_from_blob(blob)

    
#===================================================================================================
# InboxMessage, Inbox, OutboxMessage, Outbox
# These form the key utility object to work with the more unwieldy structure of mailboxes 
# and messages from grit.
#===================================================================================================
class InboxMessage:
    __loader:ObjectLoader
    sender_id:ActorId
    message_id:MessageId
    previous_id:MessageId|None
    headers:dict[str, str]
    content_id:MessageId

    def __init__(self, loader:ObjectLoader, sender_id:ActorId, message_id:MessageId, message:Message):
        self.__loader = loader
        self.sender_id = sender_id
        self.message_id = message_id
        self.previous_id = message.previous
        self.headers = message.headers.copy() if message.headers is not None else {}
        self.content_id = message.content

    @classmethod
    async def from_message_id(cls, loader:ObjectLoader, sender_id:ActorId, message_id:MessageId) -> InboxMessage:
        message = await loader.load(message_id)
        if(type(message).__name__ != 'Message'):
            raise TypeError(f"Object with id '{message_id}' is not a message, but a '{type(message).__name__}'")
        return InboxMessage(loader, sender_id, message_id, message)
    
    @property
    def mt(self) -> str:
        return self.headers.get('mt', None)
    @property
    def is_signal(self) -> bool:
        return self.previous_id is None

    async def get_content(self) -> BlobObject | TreeObject:
        content = await self.__loader.load(self.content_id)
        if(isinstance(content, dict)):
            return TreeObject(self.__loader, content, self.content_id)
        else:
            return BlobObject(content, self.content_id)
        
class InboxMessageIterator:
    __loader:ObjectLoader
    sender_id:ActorId
    head_id:MessageId
    current_id:MessageId

    def __init__(self, loader:ObjectLoader, sender_id:ActorId, head_id:MessageId):
        self.__loader = loader
        self.sender_id = sender_id
        self.head_id = head_id
        self.current_id = head_id

    def __aiter__(self):
        return self
    
    async def __anext__(self):
        if(self.current_id is None):
            raise StopAsyncIteration
        message_id = self.current_id
        msg = await InboxMessage.from_message_id(self.__loader, self.sender_id, message_id)
        self.current_id = msg.previous_id
        return msg

class Inbox:
    __loader:ObjectLoader
    __new_inbox:Mailbox
    __read_inbox:Mailbox

    def __init__(self, loader:ObjectLoader, previous_inbox:Mailbox, new_inbox:Mailbox):
        self.__loader = loader
        if(new_inbox is None):
            new_inbox = Mailbox()
        self.__new_inbox = new_inbox.copy()
        #todo: move these checks to a helper method
        if(len(self.__new_inbox) > 0):
            #make sure the dictionary is of the shape {bytes: bytes}
            first_key = next(iter(self.__new_inbox))
            if(not isinstance(first_key, bytes) or not isinstance(self.__new_inbox[first_key], bytes)):
                raise TypeError("Actor id and message id must be bytes'")
        if(previous_inbox is None):
            previous_inbox = Mailbox()
        self.__read_inbox = previous_inbox.copy() #no messages read yet: read_inbox = previous_inbox 

    @classmethod
    async def from_inbox_id(cls, loader:ObjectLoader, previous_inbox_id:MailboxId, new_inbox:Mailbox) -> 'Inbox':
        previous_inbox = await loader.load(previous_inbox_id)
        return Inbox(loader, previous_inbox, new_inbox)

    async def read_new_from_sender(self, sender_id:ActorId, n:int=-1) -> list[InboxMessage]:
        messages = []
        #see if there are messages from that actor
        if(sender_id not in self.__new_inbox):
            return messages
        #find last read message from that actor (in previous inbox)
        last_read = None
        if(sender_id in self.__read_inbox):
            last_read = self.__read_inbox[sender_id]
        #read all messages from that actor up to the last read message
        async for msg in InboxMessageIterator(self.__loader, sender_id, self.__new_inbox[sender_id]):
            if(last_read is not None and msg.message_id == last_read):
                break
            messages.append(msg)
        #messages were read backwards, as a linked list, so reverse them
        messages.reverse()
        if(n > 0):
            messages = messages[:n]
        #update read inbox to point to the last message read
        if(len(messages) > 0):
            self.__read_inbox[sender_id] = messages[-1].message_id
        #Note: checking that all messages are actually sent to this actor, 
        # i.e., that they are valid, is done by the runtime
        return messages
    
    async def read_new(self, n_from_each_sender:int=-1) -> list[InboxMessage]:
        # if(self.__read_inbox == None or len(self.__read_inbox)):
        #     return []
        #read the messages concurrently
        message_futures = []
        for actor_id in self.__new_inbox:
            message_futures.append(self.read_new_from_sender(actor_id, n_from_each_sender))
        message_lists = await asyncio.gather(*message_futures, return_exceptions=True)
        messages = []
        for message_list in message_lists:
            messages.extend(message_list)
        return messages
    
    async def read_every_from_sender(self, sender_id:ActorId) -> list[InboxMessage]:
        messages = []
        #see if there are messages from that actor
        if(sender_id not in self.__new_inbox):
            return messages
        #read all messages from that actor
        async for msg in InboxMessageIterator(self.__loader, sender_id, self.__new_inbox[sender_id]):
            messages.append(msg)
        #messages were read backwards, as a linked list, so reverse them
        messages.reverse()
        #update read inbox to point to the last message read
        if(len(messages) > 0):
            self.__read_inbox[sender_id] = messages[-1].message_id
        return messages
    
    def set_read_manually(self, sender_id:ActorId, message_id:MessageId):
        self.__read_inbox[sender_id] = message_id

    def get_current(self) -> Mailbox:
        # the current inbox is how far the inbox has been read, so the "read_inbox"
        return self.__read_inbox.copy()

    def is_empty(self) -> bool:
        return len(self.__read_inbox) == 0
    
    async def persist(self, store:ObjectStore=None) -> MailboxId:
        if(self.is_empty()):
            raise Exception("Cannot persist empty inbox, at least one message must be present")
        #if store is None, see if the __loader is an ObjectStore instance
        if(store is None):
            if(isinstance(self.__loader, ObjectStore)):
                store = self.__loader
            else:
                raise Exception("Cannot persist inbox, no ObjectStore instance provided, "+
                                "tried to see if the loader is an ObjectStore instance, but it is not.")
        return await store.store(self.__read_inbox)

ValidMessageContent = BlobId | TreeId | BlobObject | TreeObject | str | bytes | dict | BaseModel

class OutboxMessage:
    __previous_id:MessageId|None
    __is_signal:bool
    recipient_id:ActorId
    content: BlobId | TreeId | BlobObject | TreeObject | None
    headers:dict[str, str]

    def __init__(self, recipient_id:ActorId, is_signal:bool=False):
        self.__previous_id = None
        self.__is_signal = is_signal
        self.recipient_id = recipient_id
        self.headers = {}

    @property
    def is_signal(self) -> bool:
        return self.__is_signal
    
    @is_signal.setter
    def is_signal(self, value:bool):
        self.__is_signal = value
        if(value): #if it is a signal, it cannot have a previous message
            self.__previous_id = None

    @property
    def previous_id(self) -> MessageId|None:
        return self.__previous_id
    
    @previous_id.setter
    def previous_id(self, value:MessageId|None):
        '''If this is a queued message, it will usually be set when the message is processed when the outbox is persisted.
        Only set this manually if you wish to manage the message chain yourself. For example, in an event streaming setup.'''
        if(self.__is_signal):
            raise Exception("Cannot set previous message for signal")
        self.__previous_id = value

    @property
    def mt(self) -> str:
        return self.headers.get("mt")
    
    @mt.setter
    def mt(self, value:str):
        self.headers["mt"] = value

    def is_empty(self) -> bool:
        return self.content is None

    async def persist(self, store:ObjectStore) -> MessageId:
        if(self.is_empty()):
            raise Exception("Cannot persist empty message, content must be set")
        if(is_object_id(self.content)):
            content_id = self.content
        else:
            content_id = await self.content.persist(store)
        headers = self.headers if self.headers is not None and len(self.headers) > 0 else None
        msg = Message(self.__previous_id, headers, content_id)
        msg_id = await store.store(msg)
        return msg_id
    
    async def persist_to_mailbox_update(self, store:ObjectStore, sender_id:ActorId) -> tuple[ActorId, ActorId, MessageId]:
        message_id = await self.persist(store)
        return (sender_id, self.recipient_id, message_id)

    @classmethod
    async def from_genesis(cls, store:ObjectStore, core:TreeObject) -> OutboxMessage:
        #need to persist the core, so that the content id is known, which defines the recipient
        # in a genesis message, the content id and recipient id are the same
        gen_content_id = await core.persist(store)
        msg = cls(gen_content_id, is_signal=False)
        msg.content = gen_content_id
        msg.headers["mt"] = "genesis"
        return msg
    
    @classmethod
    def from_update(cls, recipient_id:ActorId, core:TreeObject) -> OutboxMessage:
        msg = cls(recipient_id, is_signal=False)
        msg.content = _ensure_content(core)
        msg.headers["mt"] = "update"
        return msg

    @classmethod
    def from_new(cls, recipient_id:ActorId, content:ValidMessageContent, is_signal:bool=False, mt:str|None=None) -> OutboxMessage:
        msg = cls(recipient_id, is_signal)
        msg.content = _ensure_content(content)
        if(mt is not None):
            msg.mt = mt
        return msg
    
    @classmethod
    def from_reply(cls, reply_to:InboxMessage, content:ValidMessageContent, is_signal:bool=False, mt:str|None=None) -> OutboxMessage:
        msg = cls(reply_to.sender_id, is_signal)
        msg.content = _ensure_content(content)
        msg.headers["reply"] = reply_to.message_id.hex()
        if(mt is not None):
            msg.mt = mt
        return msg

def _ensure_content(content:ValidMessageContent):
    if(content is None):
        raise ValueError("Content cannot be None")
    elif(is_object_id(content)):
        return content
    elif(isinstance(content, str)):
        return BlobObject.from_str(content)
    elif(isinstance(content, bytes)):
        return BlobObject.from_bytes(content)
    elif(isinstance(content, dict)):
        return BlobObject.from_json(content)
    elif(isinstance(content, BlobObject) or isinstance(content, TreeObject)):
        return content
    elif(isinstance(content, BaseModel)):
        return BlobObject.from_json(content)
    else:
        raise Exception("Invalid content type")

class Outbox:
    __new_outbox:Mailbox
    __outbox_buffer:dict[ActorId, OutboxMessage | list[OutboxMessage]] #if it's a signal, it's not a list; if it's queued, it's a list

    def __init__(self, outbox:Mailbox):
        if(outbox is None):
            outbox = Mailbox()
        #build the new outbox from the previous outbox
        self.__new_outbox = outbox.copy()
        self.__outbox_buffer = {}

    @classmethod
    async def from_outbox_id(cls, loader:ObjectLoader, outbox_id:MailboxId) -> 'Outbox':
        outbox = await loader.load(outbox_id)
        return Outbox(outbox)

    def add(self, message:OutboxMessage):
        '''Adds a message to the outbox buffer. It does not yet persist it.
        Messages can be signal messages, which do not have a previous message, or queued messages, which do have a previous message.
        The runtime delivers them differently to the recipient. Signals can be interrupt a running actor, while queued messages are
        delivered in order, and only after other queued messages have been processed.'''
        if(not isinstance(message, OutboxMessage) and type(message).__name__ != 'OutboxMessage'):
            raise Exception(f"Invalid message type, expected OutboxMessage, got {type(message)}")
        if(message.is_empty()):
            raise Exception("Cannot add empty message to outbox, content must be set")
        if(message.is_signal):
            if(message.recipient_id in self.__outbox_buffer and isinstance(self.__outbox_buffer[message.recipient_id], list)):
                raise Exception("Cannot add signal message to queue outbox (a message was probably added with is_signal=False before), "+
                                "please clear the outbox for that recipient first")
            self.__outbox_buffer[message.recipient_id] = message
        else:
            if(message.recipient_id not in self.__outbox_buffer):
                self.__outbox_buffer[message.recipient_id] = []
            elif(not isinstance(self.__outbox_buffer[message.recipient_id], list)):
                raise Exception("Cannot add queued message to signal outbox (a message was probably added with is_signal=True before), "+
                                "please clear the outbox for that recipient first")
            self.__outbox_buffer[message.recipient_id].append(message)


    def add_new_msg(self, recipient_id:ActorId, content:ValidMessageContent, is_signal:bool=False, mt:str|None=None) -> OutboxMessage:
        msg = OutboxMessage.from_new(recipient_id, content, is_signal, mt)
        self.add(msg)
        return msg
    
    def add_reply_msg(self, reply_to:InboxMessage, content:ValidMessageContent, is_signal:bool=False, mt:str|None=None) -> OutboxMessage:
        msg = OutboxMessage.from_reply(reply_to, content, is_signal, mt)
        self.add(msg)
        return msg
    

    def get_buffer(self, recipient_id:ActorId) -> list[OutboxMessage]:
        if(recipient_id in self.__outbox_buffer):
            buffer = self.__outbox_buffer[recipient_id]
            if(isinstance(buffer, list)):
                return buffer
            else:
                return [buffer]
        return []
    
    def clear_buffer(self, recipient_id:ActorId):
        if(recipient_id in self.__outbox_buffer):
            del self.__outbox_buffer[recipient_id]

    def clear_all_buffers(self):
        self.__outbox_buffer = {}

    def remove_recipient(self, recipient_id:ActorId):
        '''Remove a recipient entirely from the outbox, even if no no message was added this step. 
        This indicates that this actor will send no more messages to that recipient.
        If just the current messages in the buffer should be removed, use clear_buffer() instead.'''
        if(recipient_id in self.__new_outbox):
            del self.__new_outbox[recipient_id]
        if(recipient_id in self.__outbox_buffer):
            del self.__outbox_buffer[recipient_id]

    def get_current(self) -> Mailbox:
        '''Get a snapshot of the outbox to the extent that it has been persisted. This does not include messages added to the buffer.'''
        # the new outbox is how far the outbox has been persisted
        return self.__new_outbox.copy()

    def is_empty(self) -> bool:
        if len(self.__new_outbox) == 0 and len(self.__outbox_buffer) == 0:
            return True
        #if anything has been added to the buffer, the outbox is not considered empty
        return False

    async def __persist_recipient_buffer(self, store:ObjectStore, recipient_id:ActorId) -> MessageId:
        #check if the recipient buffer is a signal or queue
        #if it's not a list, assume its a signal message
        if(not isinstance(self.__outbox_buffer[recipient_id], list)):
            #if it's a signal, just persist the message
            message = self.__outbox_buffer[recipient_id]
            if(not message.is_signal):
                raise Exception("Trying to persist a queue message as if it is a signal message. Outbox state is inconsistent.")
            message_id = await message.persist(store)
            #and update the outbox
            self.__new_outbox[recipient_id] = message_id
            del self.__outbox_buffer[recipient_id]
            return message_id
        else:
            #if it's a queue, the message chain/queue should not be managed manually, by the user, 
            # and so all the previous_ids should be None (they will be set below)
            # (it does work, however, if the queue has only a single message in it).
            messages:list[OutboxMessage] = self.__outbox_buffer[recipient_id]
            if(len(messages) > 1 and any(message.previous_id is not None for message in messages)):
                raise Exception("For some queued messags the previous message id was set, "+
                                "the Inbox class does not support manually managed message chains for more than one message.")
            #now, save the queue for that recipient step by step, so that each message id can be linked to the previous one
            last_message_id = None
            #get the last known sent message id for that recipient actor
            if(recipient_id in self.__new_outbox):
                last_message_id = self.__new_outbox[recipient_id]
            #iterate through the buffer queue and store each message
            message:OutboxMessage
            for message in self.__outbox_buffer[recipient_id]:
                if(message.is_signal):
                    raise Exception("Trying to persist a signal message as if it is a queue message. Outbox state is inconsistent.")
                if(message.previous_id is None): #already checked above if this queue, as a whole, is valid
                    message.previous_id = last_message_id
                message_id = await message.persist(store)
                last_message_id = message_id
            #update the last known sent message id for that recipient actor
            self.__new_outbox[recipient_id] = last_message_id
            #clear the queue for that recipient (since the content was persisted)
            del self.__outbox_buffer[recipient_id]
            return last_message_id
    
    async def persist(self, store:ObjectStore) -> MailboxId:
        if(self.is_empty()):
            raise Exception("Cannot persist empty outbox, at least one message must be present")
        #save each queue concurrently
        queue_futures = []
        for recipient in list(self.__outbox_buffer):
            queue_futures.append(self.__persist_recipient_buffer(store, recipient))
        await asyncio.gather(*queue_futures)
        #the latest message id in the new_outbox was already set in __persist_recipient_buffer
        # so it can be persisted as-is
        return await store.store(self.__new_outbox)