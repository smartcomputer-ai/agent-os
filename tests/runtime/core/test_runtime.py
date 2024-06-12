import os

from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.grit import *
from aos.wit import *
from aos.runtime.core import *
import helpers_runtime as helpers

async def test_run_empty():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    runtime = Runtime(store, refs, point=1000)
    running_task = asyncio.create_task(runtime.start())
    await asyncio.sleep(0.1)
    runtime.stop()
    await running_task

    #there should be the agent actor (representing the runtime)
    agent_id = await refs.get(ref_root_actor())
    assert agent_id is not None
    agent_core = await store.load(agent_id)
    assert agent_core is not None
    assert "point" in agent_core
    assert bytes_to_point((await BlobObject.from_blob_id(store, agent_core['point'])).get_as_bytes()) == 1000
    # a step was created for he agent actor
    head = await refs.get(ref_step_head(agent_id))
    assert head is not None


