import os
import time
import importlib

import pytest
from aos.wit import *
from aos.grit import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit.data_model import *
from aos.runtime import *
from aos.runtime.core.core_loader import *
import helpers_runtime as helpers

helper_py = """
def help():
    return "helper"
"""

main_py_absolute = """
import helper
def main():
    print(helper.help())
    return helper.help()
"""

main_py_relative = """
from . import helper
def main():
    print(helper.help())
    return helper.help()
"""

main_py_submodule_relative = """
from .helperlib import helper
def main():
    print(helper.help())
    return helper.help()
"""

helper_innit_py = """
from . helper import help
"""

main_py_submodule_with_init_relative = """
from helperlib import *
def main():
    print(help())
    return help()
"""

#this doesn't work, see below in the relevant unit test
main_py_submodule_absolute = """
import helperlib.helper as h
def main():
    print(h.help())
    return h.help()
"""

def execute_module_and_assert(store:ObjectStore, core_id:TreeId):
    CoreLoader.add_to_meta_path(store)
    #since the code is under the /code node, that needs to be the entry point
    core:Tree = store.load_sync(core_id)
    code_id = core["code"]
    module = importlib.import_module(".main", code_id.hex())
    assert hasattr(module, "main")
    assert module.main() == "helper"

def print_modules():
    for m in sys.modules:
        if(hasattr(sys.modules[m], "__name__")):
            name = sys.modules[m].__name__
            if(is_object_id_str(name.split(".")[0])):
                m_type = ""
                if(hasattr(sys.modules[m], '__path__') and len(sys.modules[m].__spec__.submodule_search_locations) > 0):
                    print(sys.modules[m].__spec__.submodule_search_locations)
                    m_type = "NAMESPACE PACKAGE"
                elif hasattr(sys.modules[m], '__path__'):
                    m_type = "PACKAGE"
                else:
                    m_type = "MODULE"
                print(name, m_type)

async def test_core_loader__with_absolute_import():
    store = MemoryObjectStore()
    core = Core(store, {}, None)
    code = core.maket("code")
    code.makeb("main.py").set_as_str(main_py_absolute)
    code.makeb("helper.py").set_as_str(helper_py)
    core_id = await core.persist()
    execute_module_and_assert(store, core_id)

async def test_core_loader__with_relative_import():
    store = MemoryObjectStore()
    core = Core(store, {}, None)
    code = core.maket("code")
    code.makeb("main.py").set_as_str(main_py_relative)
    code.makeb("helper.py").set_as_str(helper_py)
    core_id = await core.persist()
    execute_module_and_assert(store, core_id)

async def test_core_loader__with_submodule_relative_import():
    store = MemoryObjectStore()
    core = Core(store, {}, None)
    code = core.maket("code")
    code.maket("helperlib").makeb("helper.py").set_as_str(helper_py)
    code.makeb("main.py").set_as_str(main_py_submodule_relative)
    core_id = await core.persist()
    execute_module_and_assert(store, core_id)

async def test_core_loader__with_submodule_and_init_relative_import():
    store = MemoryObjectStore()
    core = Core(store, {}, None)
    code = core.maket("code")
    code.maket("helperlib").makeb("helper.py").set_as_str(helper_py)
    code.maket("helperlib").makeb("__init__.py").set_as_str(helper_innit_py)
    code.makeb("main.py").set_as_str(main_py_submodule_with_init_relative)
    core_id = await core.persist()
    execute_module_and_assert(store, core_id)
    #print_modules()

async def test_core_loader__with_submodule_absolute_import_error():
    # abosulte imports with submodules do not work, rn
    # becasue the import system expects the initial module (foo in foo.bar) 
    # to be registered WITHOUT the tree id prefix
    store = MemoryObjectStore()
    core = Core(store, {}, None)
    code = core.maket("code")
    code.maket("helperlib").makeb("helper.py").set_as_str(helper_py)
    code.makeb("main.py").set_as_str(main_py_submodule_absolute)
    core_id = await core.persist()
    # the problem manifests as a "KeyError" in the importlib machinery
    with pytest.raises(KeyError) as e:
        execute_module_and_assert(store, core_id)