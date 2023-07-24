import time
from src.wit import *
from src.grit.stores.memory import MemoryObjectStore, MemoryReferences
from src.wit.data_model import *
from src.runtime import *
import helpers_runtime as helpers

# A broader end-to-end test with fanout pattern:
# 1) one actor gets created (wit_a) 
# 2) and then wit_a creates ~100 more actors (wit_b), and they all send messages back to the first wit

async def test_msgs__fanout():
    actors = set()
    messages_from_senders = {}
    roundtrip_times = set()

    wit_a = Wit()
    @wit_a.run_wit
    async def wit_a_func(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
        #print('wit_a')
        object_store:ObjectStore = kwargs['object_store']
        actor_id:ActorId = kwargs['actor_id']
        actors.add(actor_id)

        inbox_messages = await inbox.read_new()
        for message in inbox_messages:
            #if genesis message
            if(message.content_id == actor_id):
                for i in range(100):
                    actor_core_b_n = Core.from_external_wit_ref(object_store, 'wit_b')
                    actor_core_b_n.maket('data').makeb('args').set_as_json({'number': i})
                    outbox.add(await OutboxMessage.from_genesis(object_store, actor_core_b_n))
            else:
                messages_from_senders.setdefault(message.sender_id, 0)
                messages_from_senders[message.sender_id] += 1
                roundtrip_times.add(time.time())

    wit_b = Wit()
    @wit_b.run_wit
    async def wit_b_func(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
        #print('wit_b')
        actor_id:ActorId = kwargs['actor_id']
        actors.add(actor_id)

        inbox_messages = await inbox.read_new()
        for message in inbox_messages:
            #if genesis message
            if(message.content_id == actor_id):
                #await asyncio.sleep(0.01)
                messages_from_senders.setdefault(message.sender_id, 0)
                messages_from_senders[message.sender_id] += 1
                #send a message back
                outbox.add(OutboxMessage.from_reply(message, "hello from wit_b"))

    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('wit_a', wit_a)
    resolver.register('wit_b', wit_b)
    runtime = Runtime(store, refs, "test", resolver)

    running_task = asyncio.create_task(runtime.start())
    await asyncio.sleep(0.05)
    start_time = time.time()
    wit_a_gen_msg = await helpers.create_genesis_message(store, runtime.agent_id, 'wit_a')
    await runtime.inject_mailbox_update(wit_a_gen_msg)
    await asyncio.sleep(0.2)
    runtime.stop()
    await asyncio.wait_for(running_task, timeout=1) 

    #gett the max time in roundtrip_times
    max_time = max(roundtrip_times)
    print(f'max roundtrip time: {max_time - start_time}')
    min_time = min(roundtrip_times)
    print(f'min roundtrip time: {min_time - start_time}')

    # print(actors)
    # print(messages_from_senders)
    assert len(actors) == 101
    assert len(messages_from_senders) == 101





