import os

from aos.grit import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit import *
from aos.runtime.core.runtime import *
from aos.runtime.core.actor_executor import ActorExecutor
import helpers_runtime as helpers
    

async def test_run_empty():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    actor_id = helpers.get_random_actor_id()
    gen_exec = ActorExecutor.from_genesis(ExecutionContext.from_store(store, refs, resolver, helpers.get_random_actor_id()), actor_id)

    callbacks = 0
    async def outbox_callback(outbox:Mailbox):
        callbacks += 1

    run_task = asyncio.create_task(gen_exec.start(outbox_callback))
    await asyncio.sleep(0.2)
    gen_exec.stop()
    await run_task

    assert callbacks == 0

async def test_run_with_genesis_message():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    wit = Wit()
    wit.run_wit(wit_simple)
    resolver = ExternalResolver(store)
    actor_id = helpers.get_random_actor_id()
    resolver.register("wit_simple", wit)
 
    # create a random 'actor' sender_id
    sender_id = helpers.get_random_actor_id()
    sender_id, actor_id, gen_message_id = await helpers.create_genesis_message(store, sender_id, "wit_simple")
    
    gen_exec = ActorExecutor.from_genesis(ExecutionContext.from_store(store, refs, resolver, helpers.get_random_actor_id()), actor_id)
 
    outbox_callback_count = 0
    async def outbox_callback(outbox:Mailbox):
        outbox_callback_count += 1
 
    run_task = asyncio.create_task(gen_exec.start(outbox_callback))
    await gen_exec.update_current_inbox([(sender_id, actor_id, gen_message_id)])
    # let the task run for a bit
    await asyncio.sleep(0.2)
    gen_exec.stop()
    await run_task
 
    # did not produce an outbox callback
    assert outbox_callback_count == 0
    # test that a step got created inbox was updated
    step_head_id = await refs.get(ref_step_head(gen_exec.actor_id))
    assert step_head_id is not None
    step:Step = await store.load(step_head_id)
    assert step.previous is None # since it was the fist step, there should be no previous
    assert step.actor is not None
    assert step.inbox is not None
    assert step.outbox is None # no out messge has been sent yet
    assert step.core is not None
    step_inbox:Mailbox = await store.load(step.inbox)
    assert len(step_inbox) == 1
    assert sender_id in step_inbox
    assert step_inbox[sender_id] == gen_message_id

async def test_run_with_many_messages():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    wit = Wit()
    wit.run_wit(wit_simple)
    resolver = ExternalResolver(store)
    resolver.register("wit_simple", wit)
    actor_id = helpers.get_random_actor_id()
 
    # create 3 random 'actor' sender_ids
    sender_ids = [helpers.get_random_actor_id(), helpers.get_random_actor_id(), helpers.get_random_actor_id()]
    sender_id, actor_id, gen_message_id = await helpers.create_genesis_message(store, sender_ids[0], "wit_simple")
    
    gen_exec = ActorExecutor.from_genesis(ExecutionContext.from_store(store, refs, resolver, helpers.get_random_actor_id()), actor_id)
 
    outbox_callback_count = 0
    async def outbox_callback(outbox:Mailbox):
        outbox_callback_count += 1
 
    run_task = asyncio.create_task(gen_exec.start(outbox_callback))
    await gen_exec.update_current_inbox([(sender_id, actor_id, gen_message_id)])
    # let the task run for a bit
    await asyncio.sleep(0.1)

    # start a loop where we send messages to the actor
    previous_message_ids = {sender_ids[0]: gen_message_id, sender_ids[1]: None, sender_ids[2]: None}
    for i in range(0, 100):
        sender_id = sender_ids[i % 3]
        sender_id, actor_id, message_id = await helpers.create_new_message(store, sender_ids[i % 3], actor_id, previous_message_ids[sender_id], f"message {i+1}")
        previous_message_ids[sender_id] = message_id
        await gen_exec.update_current_inbox([(sender_id, actor_id, message_id)])
        #let the taks do some work
        if(i % 3 == 0):
            await asyncio.sleep(0.001)

    await asyncio.sleep(0.1)
    gen_exec.stop()
    await run_task

    #make sure the last steps' inbox matches the previous message ids
    step_head_id = await refs.get(ref_step_head(gen_exec.actor_id))
    assert step_head_id is not None
    step:Step = await store.load(step_head_id)
    step_inbox:Mailbox = await store.load(step.inbox)
    assert len(step_inbox) == 3
    for sender_id, message_id in step_inbox.items():
        assert previous_message_ids[sender_id] == message_id


async def wit_simple(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
    counter_blob = (await (await core.gett('data')).getb('counter'))
    counter_str = counter_blob.get_as_str()
    if(counter_str is None):
        counter_str = '0'
    counter = int(counter_str)
    messages = await inbox.read_new()
    print('wit_simple messages: ' + str(len(messages)))
    for message in messages:
        if(message.content_id == kwargs['actor_id']):
            print('wit_simple genesis message')
        else:
            print('wit_simple message: ' + str(message.content_id))
    counter += len(messages)
    #print('counter: ' + str(counter))
    counter_blob.set_as_str(str(counter))


