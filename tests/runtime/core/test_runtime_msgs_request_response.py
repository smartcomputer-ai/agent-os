from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit import *
from aos.runtime.core import *
import helpers_runtime as helpers

# test wit a communicating with wit b via the "request-response" helper

async def test_msgs__single_wit():
    arrived_messages = []

    wit_a = Wit()
    @wit_a.message("start")
    async def on_a_message(actor_b:str, ctx:MessageContext) -> None:
        print("on_a_message: start")
        actor_b_id = to_object_id(actor_b)
        response = await ctx.request_response.run(OutboxMessage.from_new(actor_b_id, "hi", is_signal=True, mt="hi"), ['response'], 0.1)
        response_str = (await response.get_content()).get_as_str()
        arrived_messages.append(response_str)

    wit_b = Wit()
    @wit_b.message("hi")
    async def on_b_message(message:InboxMessage, ctx:MessageContext) -> None:
        print("on_b_message: request-response")
        ctx.outbox.add_reply_msg(message, "yo", mt="response")

    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('wit_a', wit_a)
    resolver.register('wit_b', wit_b)

    runtime = Runtime(store, refs, resolver=resolver)
    running_task = asyncio.create_task(runtime.start())
    #genesis
    gen_a_message = await helpers.create_genesis_message(store, runtime.agent_id, 'wit_a')
    await runtime.inject_mailbox_update(gen_a_message)
    gen_b_message = await helpers.create_genesis_message(store, runtime.agent_id, 'wit_b')
    await runtime.inject_mailbox_update(gen_b_message)
    #since the genesis message is injected as a mailbox update, it is treated as a signal, and we need to wait for it to be processed
    await asyncio.sleep(0.3)
    #say hi
    hi_message = OutboxMessage.from_new(gen_a_message[1], gen_b_message[1].hex(), mt="start")
    await runtime.inject_mailbox_update(await hi_message.persist_to_mailbox_update(store, runtime.agent_id))
    await asyncio.sleep(0.1)
    #stop
    runtime.stop()
    await asyncio.wait_for(running_task, timeout=1) 
    
    assert arrived_messages == ["yo"]
