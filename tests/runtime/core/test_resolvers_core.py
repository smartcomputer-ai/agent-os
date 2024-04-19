import os
import time
import importlib
from aos.wit import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit.data_model import *
from aos.runtime.core import *
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

async def test_resolve_from_core__with_absolute_module_import():
    store = MemoryObjectStore()
    core = Core(store, {}, None)
    code = core.maket("code")
    code.makeb("main.py").set_as_str(main_py_absolute)
    code.makeb("helper.py").set_as_str(helper_py)
    core.makeb("wit").set_as_str("/code:main:main")
    core_id = await core.persist()

    resolver = CoreResolver(store)
    func = await resolver.resolve(core_id, 'wit', True)
    assert func is not None
    assert func.__name__ == 'main'
    assert func() == 'helper'

