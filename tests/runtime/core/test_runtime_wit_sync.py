import os
import time
from aos.wit import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.grit.tree_helpers import *
from aos.wit.data_model import *
from aos.runtime.core import *

def wit_sync(last_step_id:StepId, new_inbox:Mailbox, **kwargs) -> StepId:
    print("wit_sync, called")
    store:ObjectStore = kwargs['store']
    if last_step_id is None:
        print("wit_sync: genesis")
        inbox_id = store.store_sync(new_inbox)
        step = Step(None, kwargs['actor_id'], inbox_id, None, kwargs['actor_id'])
        step_id = store.store_sync(step)
        return step_id

# utils
async def setup_runtime():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('wit_sync', wit_sync)
    runtime = Runtime(store, refs, "test", resolver=resolver)
    running_task = asyncio.create_task(runtime.start())
    await asyncio.sleep(0.05)
    return runtime, running_task

async def send_genesis_message(runtime:Runtime, wit_ref):
    gen_core = Core(runtime.store, {}, None)
    gen_core.makeb("wit").set_as_str(f"external:{wit_ref}")
    gen_message = await OutboxMessage.from_genesis(runtime.store, gen_core)
    await runtime.inject_message(gen_message)
    await asyncio.sleep(0.1)

# tests
async def test_wit_sync_genesis():
    runtime, running_task = await setup_runtime()
    await send_genesis_message(runtime, 'wit_sync')
    await asyncio.sleep(0.1)
    runtime.stop()
    await running_task
    assert len(runtime.get_actors()) == 1