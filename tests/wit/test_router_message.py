import pytest
from src.grit.stores.memory import MemoryObjectStore
from src.wit import *
import helpers_wit as helpers

async def setup_call_and_test(wit:Wit, content:str|BlobObject|TreeObject=None, mt:str=None):
    store = MemoryObjectStore()
    (kwargs, step_id, mailbox) = await helpers.setup_wit_with_dependencies(store, "test_wit_function")
    kwargs["signal"] = asyncio.Event()
    args = (step_id, mailbox)
    step_id = await wit(*args, **kwargs)
    assert step_id is not None

    #of another message is defined--beside genesis--call the wit with it
    if(content is not None):
        mailbox_update = await helpers.create_new_message(
            store, 
            helpers.get_random_actor_id(), 
            kwargs['actor_id'], 
            None, 
            content,
            mt)
        mailbox[mailbox_update[0]] = mailbox_update[2]
        args = (step_id, mailbox)
        step_id = await wit(*args, **kwargs)
        assert step_id is not None

    assert kwargs["signal"].is_set() == True

async def test_run_wit():
    wit = Wit()
    @wit.run_wit
    async def my_run_wit(inbox:Mailbox, outbox:Mailbox, core:Core, signal:asyncio.Event):
        print("my_run_wit called")
        signal.set()
    await setup_call_and_test(wit)

async def test_genesis_message():
    wit = Wit()
    @wit.genesis_message
    async def my_genesis_message(message, signal:asyncio.Event):
        print("my_genesis_message called")
        signal.set()
    await setup_call_and_test(wit)

async def test_message__simple():
    wit = Wit()
    @wit.message("hi")
    async def my_message(message, signal:asyncio.Event):
        print("my_message called")
        assert isinstance(message, InboxMessage)
        signal.set()
    await setup_call_and_test(wit, "hello", mt="hi")

async def test_message__convert_to_string():
    wit = Wit()
    @wit.message("hi")
    async def my_message(content:str, message:InboxMessage, signal:asyncio.Event):
        print("my_message called")
        assert isinstance(content, str)
        assert content == "hello"
        signal.set()
    await setup_call_and_test(wit, "hello", mt="hi")

async def test_message__convert_to_blob():
    wit = Wit()
    @wit.message("hi")
    async def my_message(content:BlobObject, message:InboxMessage, signal:asyncio.Event):
        print("my_message called")
        assert isinstance(content, BlobObject)
        assert content.get_as_str() == "hello"
        signal.set()
    await setup_call_and_test(wit, "hello", mt="hi")

async def test_message__convert_to_tree():
    wit = Wit()
    @wit.message("hi")
    async def my_message(content:TreeObject, message:InboxMessage, signal:asyncio.Event):
        print("my_message called")
        assert isinstance(content, TreeObject)
        assert (await content.getb("a")).get_as_str() == "hello"
        signal.set()
    tree = TreeObject(None, {}, None)
    tree.makeb("a").set_as_str("hello")
    await setup_call_and_test(wit, tree, mt="hi")

async def test_message__convert_to_pydantic_model():
    wit = Wit()
    class MyModel(BaseModel):
        a_str:str
        my_list:list[str]

    @wit.message("hi")
    async def my_message(content:MyModel, message:InboxMessage, signal:asyncio.Event):
        print("my_message called")
        assert isinstance(content, MyModel)
        assert content.a_str == "howdy"
        assert content.my_list == ["a", "b", "c"]
        signal.set()
    model = MyModel(a_str="howdy", my_list=["a", "b", "c"])
    blob = BlobObject.from_json(model)
    await setup_call_and_test(wit, blob, mt="hi")

async def test_message__convert_with_type_mismatch_error():
    wit = Wit()
    @wit.message("hi")
    async def my_message(content:TreeObject, message:InboxMessage, signal:asyncio.Event):
        print("my_message called")
        assert isinstance(content, TreeObject)
        assert (await content.getb("a")).get_as_str() == "hello"
        signal.set()
    try:
        await setup_call_and_test(wit, "hello", mt="hi")
        assert False
    except InvalidMessageException as e:
        assert e is not None    

async def test_method_decorator_fails():
    with pytest.raises(InvalidWitException):
        wit = Wit()
        class MyClass:
            @wit.run_wit
            async def my_messages(self, messages, signal:asyncio.Event):
                pass
    with pytest.raises(InvalidWitException):
        wit = Wit()
        class MyClass:
            @classmethod
            @wit.run_wit
            async def my_messages(cls, messages, signal:asyncio.Event):
                pass

async def test_state_parameter():
    class MyState(WitState):
        str1:str = None
        int1:str = None
        dict1:dict = None
        list1:list = None

    wit = Wit()
    @wit.message("hi")
    async def my_message(message, state:MyState, signal:asyncio.Event):
        print("my_message called")
        assert state is not None
        assert isinstance(state, MyState)
        assert isinstance(message, InboxMessage)
        signal.set()
    await setup_call_and_test(wit, "hello", mt="hi")