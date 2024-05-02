import os
import time
import pytest

from aos.wit.prototype import wrap_in_prototype
from aos.wit import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.grit.tree_helpers import *
from aos.wit.data_model import *
from aos.runtime.core import *

# A broader end-to-end test where the wits are executed from inside the core
wit_a_async = """
from aos.grit import *
from aos.wit import * 

wit = Wit()
@wit.genesis_message
async def wit_a_genesis(msg:InboxMessage) -> None:
    print('wit_a_genesis')

@wit.message("hi")
async def wit_a_hi(msg:InboxMessage, core:Core) -> None:
    print('wit_a_hi')
    content = await msg.get_content()
    print("Got message ONE: ", content.get_as_str())
    core.makeb('hi-one').set_from_blob(content)
"""

wit_a_async_updated = """
from aos.grit import *
from aos.wit import *

wit = Wit()
@wit.genesis_message
async def wit_a_genesis(msg:InboxMessage) -> None:
    print('wit_a_genesis')

@wit.message("hi")
async def wit_a_hi(msg:InboxMessage, core:Core) -> None:
    print('wit_a_hi_updated')
    content = await msg.get_content()
    print("Got message TWO: ", content.get_as_str())
    core.makeb('hi-two').set_from_blob(content)
"""

# utils
async def setup_runtime():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    runtime = Runtime(store, refs)
    running_task = asyncio.create_task(runtime.start())
    await asyncio.sleep(0.05)
    return runtime, running_task

async def send_prototype_genesis_message(runtime:Runtime, wit_function_name, wit_code):
    gen_core = TreeObject(runtime.store, {}, None)
    gen_core.makeb("wit").set_as_str(f"/code:wit:{wit_function_name}")
    code = gen_core.maket("code")
    code.makeb("wit.py").set_as_str(wit_code)
    prototype_core = wrap_in_prototype(gen_core)
    gen_message = await OutboxMessage.from_genesis(runtime.store, prototype_core)
    await runtime.inject_message(gen_message)
    await asyncio.sleep(0.1)
    return gen_message.recipient_id

async def send_prototype_update_message(runtime:Runtime, prototype_id, wit_code):
    core = Core(runtime.store, {}, None)
    code = core.maket("code")
    code.makeb("wit.py").set_as_str(wit_code)
    prototype_core = wrap_in_prototype(core)
    message = OutboxMessage.from_update(prototype_id, prototype_core)
    await runtime.inject_message(message)
    await asyncio.sleep(0.1)

async def send_create_message(runtime:Runtime, prototype_id, content):
    message = OutboxMessage.from_new(prototype_id, content, mt="create")
    await runtime.inject_message(message)
    await asyncio.sleep(0.1)

async def send_message(runtime:Runtime, actor_id, content):
    message = OutboxMessage.from_new(actor_id, content, mt="hi")
    await runtime.inject_message(message)
    await asyncio.sleep(0.1)

# tests
#@pytest.mark.skip(reason="fix later")
async def test_prototype_with_create():
    runtime, running_task = await setup_runtime()
    prototype_id = await send_prototype_genesis_message(runtime, "wit", wit_a_async)
    await send_create_message(runtime, prototype_id, "init")

    # there should be two actors now, one for the prototype and one for the created actor
    assert len(runtime.get_actors()) == 2
    actor_id = runtime.get_actors()[1]
    # send a message to the actor
    await send_message(runtime, actor_id, "howdy")

    runtime.stop()
    await running_task

    #get the core
    step_id = await runtime.references.get(ref_step_head(actor_id))
    step = await runtime.store.load(step_id)
    core_id = step.core
    blob = BlobObject(await load_blob_path(runtime.store, core_id, "hi-one"))
    #print(blob.get_as_str())
    assert blob.get_as_str() == 'howdy'

# tests
#@pytest.mark.skip(reason="fix later")
async def test_prototype_with_update():
    runtime, running_task = await setup_runtime()
    prototype_id = await send_prototype_genesis_message(runtime, "wit", wit_a_async)
    await send_create_message(runtime, prototype_id, "init")
    await send_prototype_update_message(runtime, prototype_id, wit_a_async_updated)
    # there should be two actors now, one for the prototype and one for the created actor
    assert len(runtime.get_actors()) == 2
    actor_id = runtime.get_actors()[1]
    print("actor_id: ", actor_id.hex())

    # send a message to the actor
    await send_message(runtime, actor_id, "howdy")

    runtime.stop()
    await running_task

    #get the core
    step_id = await runtime.references.get(ref_step_head(actor_id))
    step = await runtime.store.load(step_id)
    core_id = step.core
    blob = BlobObject(await load_blob_path(runtime.store, core_id, "hi-two"))
    #print(blob.get_as_str())
    assert blob.get_as_str() == 'howdy'
