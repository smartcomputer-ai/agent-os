import os
import pytest
from runtime.actor_executor import MailboxUpdate
from src.grit.stores.memory import MemoryObjectStore, MemoryReferences
from src.wit import *
from src.runtime import *
import helpers_wit as helpers

async def setup_call_and_return(wit:Wit, query_name:str, query_args:Blob|BlobObject|None) -> Blob|Tree:
    if isinstance(query_args, BlobObject):
        query_args = query_args.get_as_blob()
    store = MemoryObjectStore()
    kwargs, step_id = await helpers.setup_query_with_dependencies(store, wit, "test_wit_function", "my_wit_query")
    assert step_id is not None #the setup ran the genesis step
    #call it as a query
    query_args = (step_id, query_name, query_args)
    result = await wit(*query_args, **kwargs)
    return result

async def test_run_query():
    wit = Wit(generate_wut_query=False)
    @wit.run_query
    async def my_run_query(query_name):
        return BlobObject.from_str(query_name)
    result = await setup_call_and_return(wit, "test", None)
    assert result is not None
    assert is_blob(result)
    assert BlobObject.from_blob(result).get_as_str() == "test"

async def test_query__no_args():
    wit = Wit()
    @wit.query("test")
    async def my_query():
        return BlobObject.from_str("ho")
    result = await setup_call_and_return(wit, "test", None)
    assert result is not None
    assert is_blob(result)
    assert BlobObject.from_blob(result).get_as_str() == "ho"

async def test_query__json_args():
    wit = Wit()
    @wit.query("test")
    async def my_query(foo:str, bar:str):
        return BlobObject.from_str(foo+bar)
    result = await setup_call_and_return(wit, "test", BlobObject.from_json({"foo":"ho", "bar":"hoho"}))
    assert BlobObject.from_blob(result).get_as_str() == "hohoho"

async def test_query__blobobj_args():
    wit = Wit()
    @wit.query("test")
    async def my_query(obj:BlobObject):
        return obj.get_as_json()['foo']
    result = await setup_call_and_return(wit, "test", BlobObject.from_json({"foo":"ho", "bar":"hoho"}))
    assert BlobObject.from_blob(result).get_as_str() == "ho"

async def test_query__pydantic_model_args():
    class MyModel(BaseModel):
        a_str:str
        my_list:list[str]

    wit = Wit()
    @wit.query("test")
    async def my_query(mod:MyModel):
        return mod.a_str
    result = await setup_call_and_return(wit, "test", BlobObject.from_json({"a_str":"ho", "my_list":["a", "b", "c"]}))
    assert BlobObject.from_blob(result).get_as_str() == "ho"

async def test_query__pydantic_model_args():
    class MyModel(BaseModel):
        a_str:str
        my_list:list[str]

    wit = Wit()
    @wit.message("test-zero")
    async def my_message(mod:MyModel):
        pass
    @wit.query("test-one")
    async def my_query(mod:MyModel):
        pass
    @wit.query("test-two")
    async def my_query():
        pass
    result = await setup_call_and_return(wit, "wut", None)
    wut = BlobObject.from_blob(result).get_as_json()
    assert len(wut) > 0

async def test_query__returns_str():
    wit = Wit()
    @wit.query("test")
    async def my_query():
        return "ho"
    result = await setup_call_and_return(wit, "test", None)
    assert result is not None
    assert BlobObject.from_blob(result).get_as_str() == "ho"

async def test_query__returns_tree():
    wit = Wit()
    @wit.query("test")
    async def my_query(core:Core):
        return await core.gett("data") #was created in setup_call_and_return
    result = await setup_call_and_return(wit, "test", None)
    assert result is not None
    assert is_tree(result)
    assert "args" in result

async def test_query__returns_tree_id():
    wit = Wit()
    @wit.query("test")
    async def my_query(core:Core):
        return (await core.gett("data")).get_as_object_id() #was created in setup_call_and_return
    result = await setup_call_and_return(wit, "test", None)
    assert result is not None
    assert is_tree(result)
    assert "args" in result

async def test_query__returns_tree_id():
    wit = Wit()
    @wit.query("test")
    async def my_query(core:Core):
        return (await core.gett("data")).get_as_object_id() #was created in setup_call_and_return
    result = await setup_call_and_return(wit, "test", None)
    assert result is not None
    assert is_tree(result)
    assert "args" in result

async def test_query__returns_pydanitc_model():
    class MyModel(BaseModel):
        a_str:str
        my_list:list[str]

    wit = Wit()
    @wit.query("test")
    async def my_query(core:Core):
        return MyModel(a_str="ho", my_list=["a", "b", "c"])
    result = await setup_call_and_return(wit, "test", None)
    assert result is not None
    assert is_blob(result)
    blobObj  = BlobObject.from_blob(result)
    assert blobObj.get_header("ct") == "j"
    assert blobObj.get_as_json() == {"a_str":"ho", "my_list":["a", "b", "c"]}

