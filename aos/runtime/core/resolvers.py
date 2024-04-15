from __future__ import annotations
from abc import ABC, abstractmethod
import os
import sys
import importlib
import importlib.abc
import importlib.machinery
import types
from typing import Callable
from grit import *
from grit.tree_helpers import _load_path
from wit.errors import InvalidCoreException
from .core_loader import *

class Resolver(ABC):
    """Finds a wit, query, or update function as it is defined inside the core, in the '/wit', '/wit_query', or '/wit_update' node."""
    
    _object_store:ObjectStore

    def __init__(self, object_store:ObjectStore):
        self._object_store = object_store

    async def resolve(self, core_id:TreeId, node_name:str, is_required:bool) -> Callable:
        if(core_id is None):
            raise ValueError("core_id cannot be None")
        if not isinstance(core_id, TreeId):
            raise TypeError("core_id must be a TreeId")
        core = await self._object_store.load(core_id)
        if(not is_tree(core)):
            raise InvalidCoreException("core must be a Tree")
        if(node_name not in core):
            if(is_required):
                raise InvalidCoreException(f"Core is missing '{node_name}'")
            else:
                return None
        node_id = core[node_name]
        wit:Blob = await self._object_store.load(node_id)
        if(not is_blob(wit)):
            raise InvalidCoreException(f"Core '/{node_name}' must be a string blob")
        node_str = wit.data.decode('utf-8')
        node_str = node_str.strip()

        return await self._resolve(core_id, core, node_id, node_str)

    @abstractmethod
    async def _resolve(self, core_id:TreeId, core:Tree, node_id:BlobId, node_content:str) -> Callable:
        pass

class MetaResover(Resolver):
    def __init__(self, 
            object_store:ObjectStore,
            external_resolver:ExternalResolver|None = None,
            core_resolver:CoreResolver|None = None,
            code_resolver:CodeResolver|None = None,
            ):
        super().__init__(object_store)
        self.external_resolver = external_resolver
        self.core_resolver = core_resolver
        self.code_resolver = code_resolver

    @classmethod
    def with_all(cls, object_store:ObjectStore):
        return cls(
            object_store,
            external_resolver=ExternalResolver(object_store),
            core_resolver=CoreResolver(object_store),
            code_resolver=CodeResolver(object_store),
        )

    async def _resolve(self, core_id:TreeId, core:Tree, node_id:BlobId, node_content:str) -> Callable:
        #the node_content can be:
        # 1. a reference by wit_name or query_name
        # 2. a fully qualified module:func definition
        # 3. code that defines a function
        # if the the string starts with '/' or 'external:' then it is a reference,
        # otherwise it is code
        if(node_content.startswith('external:')):
            if(self.external_resolver is None):
                raise Exception("external_resolver was not provided")
            wit_func = await self.external_resolver._resolve(core_id, core, node_id, node_content)
        elif(node_content.startswith('/')):
            if(self.core_resolver is None):
                raise Exception("core_resolver was not provided")
            wit_func = await self.core_resolver._resolve(core_id, core, node_id, node_content)
        else:
            if(self.code_resolver is None):
                raise Exception("code_resolver was not provided")
            wit_func = await self.code_resolver._resolve(core_id, core, node_id, node_content)
        return wit_func

class ExternalResolver(Resolver):
    _func_factories:dict[str, Callable[[], Callable]]
    _func_resolve_cache:dict[ObjectId, Callable]
    _search_paths:list[str]

    def __init__(self, object_store:ObjectStore):
        super().__init__(object_store)
        self._func_factories = {}
        self._func_resolve_cache = {}
        self._search_paths = []

    def register_factory(self, ref:str, func_factory:Callable[[], Callable]):
        self._func_factories[ref] = func_factory
        pass

    def register(self, ref:str, func:Callable):
        self._func_factories[ref] = lambda: func

    def register_path(self, path:str):
        if(not os.path.isdir(path)):
            raise ValueError(f"Path '{path}' is not a directory")
        if(path not in self._search_paths):
            self._search_paths.append(path)
            if(path not in sys.path):
                sys.path.append(path)
    
    def path_reloaded(self, path:str):
        #todo: implement module reloading when a file was changed and it corresponds to an exernally loaded module
        # alternatively, make this resolver watch paths for changes and reload modules
        pass

    async def _resolve(self, core_id:TreeId, core:Tree, node_id:BlobId, node_content:str) -> Callable:
        #check if this function has been cached, otherwise load it from normal path modules
        if(node_content in self._func_resolve_cache):
            return self._func_resolve_cache[node_content]
        else:
            func = await self._resolve_with_external(node_content)
            if(func is not None):
                self._func_resolve_cache[node_content] = func
            return func

    async def _resolve_with_external(self, node_content:str) -> Callable:
        if(not node_content.startswith('external:')):
            raise ValueError("node_content must start with 'external:'")
        node_content = node_content[9:]
        #check if it's a module:func or just a wit_name
        if(':' not in node_content):
            #it's just a wit_name
            func_ref = node_content
            if(func_ref not in self._func_factories):
                raise KeyError(f"Cannot find function by reference '{func_ref}', please register function in resolver.")
            #execute the factory to create or retrieve the function
            func = self._func_factories[func_ref]()
            return func
        else:
            #it's a module:func
            module_path, func_name = node_content.split(':')
            if(module_path not in sys.modules):
                #import the module
                #TODO: the import should really have the package name to disambiguate other module names (that is, the root path of the agent code)
                # maybe easier would be to configure the a PathEntryFinder when setting up the runtime
                module = importlib.import_module(module_path)
            else:
                module = sys.modules[module_path]
            # find the function
            func = find_function(func_name, module)
            if(func is None):
                raise KeyError(f"Cannot find external function '{func_name}' in module '{module.__name__}'.")
            return func

class CoreResolver(Resolver):
    _func_resolve_cache:dict[str, Callable]

    def __init__(self, object_store:ObjectStore):
        super().__init__(object_store)
        self._func_resolve_cache = {}
        # add the core loader to the sys.meta_path (so that the modules can be loaded from the core)
        # this will only add it once, if it was already added, it is a no-op
        CoreLoader.add_to_meta_path(object_store)

    async def _resolve(self, core_id:TreeId, core:Tree, node_id:BlobId, node_content:str) -> Callable:
        if(not node_content.startswith('/')):
            raise ValueError("node_content must start with '/'")
        node_content_parts = node_content.split(':')
        if len(node_content_parts) != 3:
            raise ValueError("node_content must be in the format '/path/to/code:module:function_name'")
        code_path = node_content_parts[0].strip()
        module_path = node_content_parts[1].strip()
        func_name = node_content_parts[2].strip()
        # find the tree_id for the code path
        code_path_parts = code_path.split('/')
        code_path_parts = [p for p in code_path_parts if p != '']
        _, code_tree_id, _ = await _load_path(self._object_store, core_id, code_path_parts)
        #check the cache
        cache_key = f"{node_content}-{code_tree_id.hex()}"    
        if(cache_key in self._func_resolve_cache):
            return self._func_resolve_cache[cache_key]
        # make the module path relative, and also create a fully qualified module path
        if(not module_path.startswith('.')):
            module_path = f".{module_path}"
        module_path_full = f"{code_tree_id.hex()}{module_path}" #the dot is already part of the module_path
        # check the modules if this module was already loaded
        if(module_path_full in sys.modules):
            module = sys.modules[module_path_full]
        else:
            # import the module
            # define the root path of the module as a package which uses the tree_id as the package name
            # this will load the module using the CoreLoader
            # the resulting loaded module name will follow the convetion of the module_path_full name
            module = importlib.import_module(module_path, code_tree_id.hex())
        # find the function
        func = find_function(func_name, module)
        if(func is None):
            raise KeyError(f"Cannot find function '{func_name}' in module '{module.__name__}'.")
        # cache the function
        self._func_resolve_cache[cache_key] = func
        return func

class CodeResolver(Resolver):
    def __init__(self, object_store:ObjectStore):
        super().__init__(object_store)

    async def _resolve(self, core_id:TreeId, core:Tree, node_id:BlobId, node_content:str) -> Callable:
        #TODO execute the code and look for a function 
        # this should execute the code as a module using the CoreLoader, this allows the code to import other modules
        raise NotImplementedError()

def find_function(func_name:str, module:types.ModuleType) -> Callable|None:
    search_attr = module
    if('.' in func_name):
        #it's a nested function, or class method, or static method
        *nested_names, func_name = func_name.split('.')
        for nested_name in nested_names:
            search_attr = getattr(search_attr, nested_name)
    if(hasattr(search_attr, func_name)):
        func = getattr(search_attr, func_name)
        return func
    else:
        return None