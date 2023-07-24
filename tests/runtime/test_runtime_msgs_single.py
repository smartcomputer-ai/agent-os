from grit.stores.memory import MemoryObjectStore, MemoryReferences
from wit import *
from runtime import *
import helpers_runtime as helpers

# test a the wit function in conjunction with the runtime

async def test_msgs__single_wit():
    arrived_messages = []

    wit_a = Wit()
    @wit_a.genesis_message
    async def on_genesis_message(message:InboxMessage, actor_id) -> None:
        print(f"on_genesis_message: I am {actor_id}")
        arrived_messages.append("genesis")

    @wit_a.message("hi")
    async def on_message(message:InboxMessage, actor_id) -> None:
        print(f"on_message: I am {actor_id}")
        arrived_messages.append("hi")

    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('wit_a', wit_a)

    runtime = Runtime(store, refs, "test", resolver)
    running_task = asyncio.create_task(runtime.start())
    #genesis
    gen_message = await helpers.create_genesis_message(store, runtime.agent_id, 'wit_a')
    await runtime.inject_mailbox_update(gen_message)
    #since the genesis message is injected as a mailbox update, it is treated as a signal, and we need to wait for it to be processed
    await asyncio.sleep(0.1)
    #say hi
    hi_message = OutboxMessage.from_new(gen_message[1], "hi from outside", mt="hi")
    hi_message
    await runtime.inject_mailbox_update(await hi_message.persist_to_mailbox_update(store, runtime.agent_id))
    await asyncio.sleep(0.1)
    #stop
    runtime.stop()
    await asyncio.wait_for(running_task, timeout=1) 
    
    assert arrived_messages == ["genesis", "hi"]
