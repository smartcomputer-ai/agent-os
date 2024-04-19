from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit import *
from aos.runtime import *
import helpers_wit as helpers

class MoreData:
    my_details:str = None

class MyState(WitState):
    str1:str = None
    int1:str = None
    dict1:dict = None
    list1:list = None
    subobj1:MoreData = None

async def test_automatic_property_persistence_in_core():
    store = MemoryObjectStore()
    core = Core(store, {}, None)

    state = MyState()
    # all is empty
    assert state.str1 is None
    assert state.subobj1 is None
    await state._load_from_core(core)
    # still empty
    assert state.str1 is None
    assert state.subobj1 is None

    # set vars
    state.str1 = "str1"
    state.int1 = 100
    state.dict1 = {"a":1, "b":2}
    state.list1 = [1,2,3]
    state.subobj1 = MoreData()
    state.subobj1.my_details = "my details"
    # persist
    await state._persist_to_core(core)
    state_data = (await core.gett("state"))
    assert len(state_data) == 5

    # try wit a new object
    state = MyState()
    # empty again
    assert state.str1 is None
    assert state.subobj1 is None
    # load from the core
    await state._load_from_core(core)
    assert state.str1 is not None
    assert state.str1 == "str1"
    assert state.int1 == 100
    assert state.dict1 == {"a":1, "b":2}
    assert state.list1 == [1,2,3]
    assert state.subobj1 is not None
    assert state.subobj1.my_details == "my details"
