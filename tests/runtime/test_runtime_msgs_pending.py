from src.grit.stores.memory import MemoryObjectStore, MemoryReferences
from src.wit import *
from src.runtime import *
import helpers_runtime as helpers

# A broader end-to-end test that makes sure the runtime applies pending messages:
# 1. Key: Do not run the runtime yet
# 2. Create an actor manually (wit_a)
# 3. Manually (again without the runtime) create an outbox for actor a that
#    creates a new actor (wit_b) and sends it another message
# 4. Run the runtime
# 5. Make sure the message was received by actor b (wit_b)

async def test_msgs__runtime_pending():
    arrived_messages = []

    wit_a = Wit()
    @wit_a.run_wit
    async def wit_a_func(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
        print('wit_a')
        await inbox.read_new()

    wit_b = Wit()
    @wit_b.run_wit
    async def wit_b_func(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
        print('wit_b')
        object_store:ObjectStore = kwargs['object_store']
        actor_id:ActorId = kwargs['actor_id']
        inbox_messages = await inbox.read_new()
        #gen messages are handled individually, so there should only be ever one message at a time
        assert len(inbox_messages) == 1

        if(inbox_messages[0].content_id == actor_id):
            print('genesis message arrived')
            arrived_messages.append("genesis")
        else:
            print('other message arrived')
            arrived_messages.append("other")

    
    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('wit_a', wit_a)
    resolver.register('wit_b', wit_b)
    runtime = Runtime(store, refs, "test_agent", resolver)

    #create wit_a
    wit_a_gen_step_id = await helpers.create_actor(store, refs, runtime.agent_id, 'wit_a')
    wit_a_gen_step:Step = await store.load(wit_a_gen_step_id)
    wit_a_actor_id = wit_a_gen_step.actor

    #send two messages to wit_b, but without ever running wit_b--just send messages to it
    # to do so, manually update the outbox *of wit a* with two message (gen & howdy) and create a new step that incoroprates that outbox
    outbox = Outbox({})
    b_gen_messge = await OutboxMessage.from_genesis(store, Core.from_external_wit_ref(store, 'wit_b'))
    wit_b_actor_id = b_gen_messge.content #will be the agent id of wit_b
    outbox.add(b_gen_messge)
    outbox.add(OutboxMessage.from_new(wit_b_actor_id, "Howdy"))
    outbox_id = await outbox.persist(store)
    #inbox and core of wit_a do not change, only the outbox
    wit_a_second_step = Step(wit_a_gen_step_id, wit_a_actor_id, wit_a_gen_step.inbox, outbox_id, wit_a_gen_step.core)
    wit_a_second_step_id = await store.store(wit_a_second_step)
    await refs.set(ref_step_head(wit_a_actor_id), wit_a_second_step_id)

    #now, start the runtime
    # wit_b has never been executed so far (ie no step has been run for it), there are only outbox messages in wit_a for wit_b
    # the runtime should pick up the two messages (gen & howdy) and send them to wit_b
    running_task = asyncio.create_task(runtime.start())
    await asyncio.sleep(0.1)
    runtime.stop()
    await running_task

    assert len(arrived_messages) == 2
    assert "genesis" in arrived_messages
    assert "other" in arrived_messages

