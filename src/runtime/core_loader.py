from __future__ import annotations
from collections import OrderedDict
import sys
import types
import importlib.abc
import importlib.machinery
from grit import *
from grit.tree_helpers import _load_path_sync

_BLOB_FILE_ENDINGS = [".py"]

class CoreLoader(importlib.abc.Loader, importlib.abc.MetaPathFinder):
    """A customer importlib loader to load py modules from grit trees or cores.
    
    The loader combines two base classes: the MetaPathFinder and the Loader.
    The MetaPathFinder is used to find the module spec for a given module name,
    and is the entry point into this loader.
    The Loader is used to load and execute the module into a the actual module object.
    
    This loader does loads all modules into sys.modules (like all loaders do).
    However, it always prefixes the loaded modules with the tree id. So modules
    always look like this: '<tree_id>.<module_name>' or '<tree_id>.<package_name>.<module_name>'.

    To understand all the nuances of a loader (which are not all respected here) is advisable
    to read the importlib documentation.
    See: https://docs.python.org/3/reference/import.html
    """

    def __init__(self, object_loader:ObjectLoader):
        self.object_loader = object_loader
        self.executing_contexts = OrderedDict()

    @classmethod
    def add_to_meta_path(cls, object_loader:ObjectLoader):
        #see if sys.meta_path already has a CoreLoader, if not, add it
        for loader in sys.meta_path:
            if(isinstance(loader, CoreLoader)):
                sys.meta_path.remove(loader)
        sys.meta_path.insert(0, CoreLoader(object_loader))

    #-----------------------------------------------------------------
    # Finder
    #-----------------------------------------------------------------
    def find_spec(self, fullname:str, path:list[str]|None, target=None):
        #print("find_spec(fullname={}, path={}, target={})".format(fullname, path, target))

        # try to find the tree id in either path or fullname
        tree_id = None
        fullname_parts = fullname.strip().split('.')
        fullname_parts = [p.strip() for p in fullname_parts]
        #todo: support '..' in fullname (but not sure if this is even feasable)
        if is_object_id_str(fullname_parts[0]):
            tree_id = to_object_id(fullname_parts[0])
        
        # see if the paths contains a tree id
        # generally, though, cores don't support multi-path namespace packages 
        # so usually there will be only one possible path to search
        if tree_id is None and path is not None and len(path) > 0:
            #split each path to see if it starts with a tree id and call find_spec recursively
            for p in path:
                p_parts = p.split('/')
                if is_object_id_str(p_parts[0]):
                    tree_id = to_object_id(p_parts[0])
                    #recursively call find_spec with a modified fullname that includes the tree id
                    #print("===> find_spec recurse PATH:", p_parts[0], fullname)
                    path_spec = self.find_spec(f"{p_parts[0]}.{fullname}", None, target)
                    if path_spec is not None:
                        return path_spec

        # if tree id is still None, check the currently executing contexts
        # the assumption is that this is might be an absolute import during the exec of a module inside the tree
        if tree_id is None and len(self.executing_contexts) > 0:
            # use a set because the same tree might have been registered multiple times 
            # and we only need to search each tree once
            for exec_tree_id in set(self.executing_contexts.values()):
                exec_tree = self.object_loader.load_sync(exec_tree_id)
                if exec_tree is None or not is_tree(exec_tree):
                    continue
                if fullname in exec_tree or fullname + ".py" in exec_tree:
                    #recursively call find_spec with a modified fullname that includes the executing tree id
                    #print("===> find_spec recurse EXEC CONTEXT:", exec_tree_id.hex(), fullname)
                    #setting the path to the tree id will cause a further recurse above based on the path
                    return self.find_spec(f"{fullname}", [exec_tree_id.hex()], target)

        # if tree id is still None, return None
        if tree_id is None:
            return None
        
        # create the module spec
        # start by loading the current grit object in question
        # the path to the current grit object is all the parts in the full name, minus the tree id itself
        path_parts = fullname_parts
        if(is_object_id_str(path_parts[0])):
            path_parts = path_parts[1:]
        path_parts_str = "/".join(path_parts)
        #print("===> find_spec create SPEC:", fullname)
        _, _, obj = _load_path_sync(self.object_loader, tree_id, path_parts, _BLOB_FILE_ENDINGS)
        if obj is None:
            return None
        # now, figure out if this is a python module, a normal package, or a namespace package
        # it is a namespace package if the obj is a tree and does not contain a __init__.py
        # it is a normal package if the obj is a tree and contains a __init__.py
        # it is a python module if the obj is a blob, because we assume the blob contains code
        if(is_tree(obj)):
            # check if the tree contains a __init__.py
            if "__init__.py" in obj or "__init__" in obj:
                #print("find_spec is a normal package")
                # it's a normal package
                # and needs to be executed (the __init__ file needs to be executed)
                return importlib.machinery.ModuleSpec(fullname, self, origin=fullname, is_package=True)
            else:
                #print("find_spec is a namespace package")
                # it's a namespace package
                # namespace packages do not need to be loaded, hence loader is None
                spec = importlib.machinery.ModuleSpec(fullname, None, origin=fullname, is_package=True)
                spec.submodule_search_locations = [f"{tree_id.hex()}/{path_parts_str}"]
                return spec
        elif(is_blob(obj)):
            #print("find_spec is a module")
            # it's a python module
            # and needs to be executed
            return importlib.machinery.ModuleSpec(fullname, self, origin=fullname, is_package=False)
        else:
            raise ImportError(f"Unknown object type for {fullname}, was {type(obj)}.")
    
    #-----------------------------------------------------------------
    # Loader
    #-----------------------------------------------------------------
    def create_module(self, spec:importlib.machinery.ModuleSpec):
        #print("create_module(spec={})".format(spec))
        return None  # use default module creation

    def exec_module(self, module:types.ModuleType):
        #print("exec_module(module={})".format(module))
        #print("exec_module name", module.__name__)

        path_parts = module.__name__.split('.')
        if(not is_object_id_str(path_parts[0])):
            raise ImportError(f"Cannot exec module, module name {module.__name__} does not start with a tree id.")
        tree_id = to_object_id(path_parts[0])
        path_parts = path_parts[1:]
        path_parts_str = "/".join(path_parts)
        full_path = f"{tree_id.hex()}/{path_parts_str}"

        parent_id, obj_id, obj = _load_path_sync(self.object_loader, tree_id, path_parts, _BLOB_FILE_ENDINGS)
        #check if it is a package
        if(hasattr(module, "__path__")):
            #print("===> exec_module package")
            if(not is_tree(obj)):
                raise ImportError(f"Cannot exec package module, module name {module.__name__} is not a tree.")
            init_id = None
            if("__init__.py" in obj):
                init_id = obj["__init__.py"]
            elif("__init__" in obj):
                init_id = obj["__init__"]
            if(init_id is None):
                raise ImportError(f"Cannot exec package module, module name {module.__name__} does not contain an '__init__.py' or '__init__' blob.")
            init_obj = self.object_loader.load_sync(init_id)
            if(init_obj is None or not is_blob(init_obj)):
                raise ImportError(f"Cannot exec package module, module name {module.__name__} contains an invalid '__init__.py' or '__init__' blob.")
            self.exec_core_blob(obj_id, init_id, init_obj, module)
            module.__path__ = [full_path]
            # print("exec_module executed __init__")
            # print("module.__name__", module.__name__)
            # print("module.__path__", module.__path__)
            # print("module.__package", module.__package__)
        else:
            #print("===> exec_module module", module.__name__)
            if(not is_blob(obj)):
                raise ImportError(f"Cannot exec module, module name {module.__name__} is not a blob.")
            self.exec_core_blob(parent_id, obj_id, obj, module)
            # print("exec_module executed module")
            # print("module.__name__", module.__name__)
            # print("module.__package", module.__package__)
        #print("done exec_module")

    def exec_core_blob(self, parent_id:TreeId|None, blob_id:BlobId, blob:Blob, module:types.ModuleType):
        try:
            if parent_id is not None:
                # Register currently executing modules with the finder from the loader,
                # so that the finder can give preference to the current core with *absolute* imports. 
                self.register_executing_context(blob_id, parent_id)
            code = blob.data.decode('utf-8')
            exec(code, module.__dict__)
        finally:
            if parent_id is not None:
                self.remove_executing_context(blob_id, parent_id)

    def register_executing_context(self, blob_id:BlobId, tree_id:TreeId):
        #print(f"register_executing_context(blob_id={blob_id.hex()}, tree_id={tree_id.hex()})")
        self.executing_contexts[blob_id] = tree_id

    def remove_executing_context(self, blob_id:BlobId, tree_id:TreeId):
        #print(f"remove_executing_context(blob_id={blob_id.hex()}, tree_id={tree_id.hex()})")
        if(blob_id in self.executing_contexts):
            del self.executing_contexts[blob_id]
