import os
import time
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
@wit.run_wit
async def wit_a_async(inbox:Inbox, outbox:Outbox, core:Core) -> None:
    print('wit_a_async')
    msgs = await inbox.read_new()
    print("Messages in inbox: ", len(msgs))
"""

wit_a_async_updated = """
from aos.grit import *
from aos.wit import *

wit = Wit()
@wit.run_wit
async def wit_a_async(inbox:Inbox, outbox:Outbox, core:Core) -> None:
    print('wit_a_async UPDATED')
    msgs = await inbox.read_new()
    print("Messages in inbox: ", len(msgs))
    for msg in msgs:
        if msg.mt == 'hi':
            content = await msg.get_content()
            print("Got message: ", content.get_as_str())
            core.makeb('hi').set_from_blob(content)
"""

# utils
async def setup_runtime():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    runtime = Runtime(store, refs)
    running_task = asyncio.create_task(runtime.start())
    await asyncio.sleep(0.05)
    return runtime, running_task

async def send_genesis_message(runtime:Runtime, wit_function_name, wit_code):
    gen_core = Core(runtime.store, {}, None)
    gen_core.makeb("wit").set_as_str(f"/code:wit:{wit_function_name}")
    code = gen_core.maket("code")
    code.makeb("wit.py").set_as_str(wit_code)
    gen_message = await OutboxMessage.from_genesis(runtime.store, gen_core)
    await runtime.inject_message(gen_message)
    await asyncio.sleep(0.1)

async def send_update_message(runtime:Runtime, wit_code):
    core = Core(runtime.store, {}, None)
    code = core.maket("code")
    code.makeb("wit.py").set_as_str(wit_code)
    message = OutboxMessage.from_update(runtime.get_actors()[0], core)
    await runtime.inject_message(message)
    await asyncio.sleep(0.1)

async def send_message(runtime:Runtime, content):
    message = OutboxMessage.from_new(runtime.get_actors()[0], content)
    message.mt = "hi"
    await runtime.inject_message(message)
    await asyncio.sleep(0.1)

# tests
async def test_incore_wit_async_genesis():
    runtime, running_task = await setup_runtime()
    await send_genesis_message(runtime, "wit", wit_a_async)
    runtime.stop()
    await running_task
    assert len(runtime.get_actors()) == 1

async def test_incore_wit_async_update():
    runtime, running_task = await setup_runtime()
    await send_genesis_message(runtime, "wit", wit_a_async)
    await send_update_message(runtime, wit_a_async_updated)
    runtime.stop()
    await running_task
    #asserts
    actors = runtime.get_actors()
    assert len(actors) == 1
    #get the core
    step_id = await runtime.references.get(ref_step_head(actors[0]))
    step = await runtime.store.load(step_id)
    core_id = step.core
    blob = BlobObject(await load_blob_path(runtime.store, core_id, "code/wit.py"))
    print(blob.get_as_str())
    assert blob.get_as_str() == wit_a_async_updated

async def test_incore_wit_async_update_and_send_message():
    runtime, running_task = await setup_runtime()
    await send_genesis_message(runtime, "wit", wit_a_async)
    await send_update_message(runtime, wit_a_async_updated)
    await send_message(runtime, "howdy")
    runtime.stop()
    await running_task
    #asserts
    actors = runtime.get_actors()
    assert len(actors) == 1
    #get the core
    step_id = await runtime.references.get(ref_step_head(actors[0]))
    step = await runtime.store.load(step_id)
    core_id = step.core
    blob = BlobObject(await load_blob_path(runtime.store, core_id, "hi"))
    print(blob.get_as_str())
    assert blob.get_as_str() == 'howdy'