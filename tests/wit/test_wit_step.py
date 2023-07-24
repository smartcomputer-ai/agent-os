from src.grit.stores.memory import MemoryObjectStore, MemoryReferences
from src.grit import *
from src.wit import *
import helpers_wit as helpers

# end-to-end test of the raw wit function: can it compute steps properly

async def wit_simple(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
    messages = await inbox.read_new()
    print('wit_simple messages: ' + str(len(messages)))
    for message in messages:
        if(message.content_id == kwargs['actor_id']):
            print('wit_simple genesis message')
        else:
            content:BlobObject = await message.get_content()
            content_str = content.get_as_str()
            print('wit_simple message: ' + content_str)
            if(content_str == 'say hello'):
                outbox.add(OutboxMessage.from_reply(message, 'hello back'))
    
async def test_genesis_step():
    store = MemoryObjectStore()
    kwargs, last_step_id, new_messages = await helpers.setup_wit_with_dependencies(store, "wit_simple")
    assert len(new_messages) == 1
    sender_id = next(iter(new_messages))

    #run the wit function
    wit = Wit()
    wit.run_wit(wit_simple)
    new_step_id = await wit(*(last_step_id, new_messages), **kwargs)

    #check the new step
    step = await store.load(new_step_id)
    #print(step)
    assert step.previous is None
    assert step.actor is not None
    assert step.inbox is not None
    assert step.outbox is None
    assert step.core is not None
    #make sure the read inbox is correct (that one gen message was read)
    step_inbox:Mailbox = await store.load(step.inbox)
    assert len(step_inbox) == 1
    assert sender_id in step_inbox
    assert step_inbox[sender_id] == new_messages[sender_id]

async def test_next_step_with_two_messages():
    
    store = MemoryObjectStore()
    kwargs, last_step_id, new_messages = await helpers.setup_wit_with_dependencies(store, "wit_simple")
    assert len(new_messages) == 1
    sender_id = next(iter(new_messages))

    #run the wit function once
    wit = Wit()
    wit.run_wit(wit_simple)
    new_step_id = await wit(*(last_step_id, new_messages), **kwargs)

    #run the wit function again, with a new message
    sender_id, actor_id, new_message_id = await helpers.create_new_message(store, sender_id, kwargs['actor_id'], new_messages[sender_id], 'hello world')
    new_messages = {sender_id: new_message_id}
    new_step_id = await wit(*(new_step_id, new_messages), **kwargs)
    
    #check the new step
    step = await store.load(new_step_id)
    assert step.previous is not None
    assert step.actor is not None
    assert step.inbox is not None
    assert step.outbox is None
    assert step.core is not None
    step_inbox:Mailbox = await store.load(step.inbox)
    assert len(step_inbox) == 1
    assert sender_id in step_inbox
    assert step_inbox[sender_id] == new_messages[sender_id]

async def test_next_step_with_outbox_message():
    
    store = MemoryObjectStore()
    kwargs, last_step_id, new_messages = await helpers.setup_wit_with_dependencies(store, "wit_simple")
    assert len(new_messages) == 1
    sender_id = next(iter(new_messages))

    #run the wit function once
    wit = Wit()
    wit.run_wit(wit_simple)
    new_step_id = await wit(*(last_step_id, new_messages), **kwargs)

    #run the wit function again, with a new message
    sender_id, actor_id, new_message_id = await helpers.create_new_message(store, sender_id, kwargs['actor_id'], new_messages[sender_id], 'say hello')
    new_messages = {sender_id: new_message_id}
    new_step_id = await wit(*(new_step_id, new_messages), **kwargs)
    
    #check the new step
    step = await store.load(new_step_id)
    assert step.previous is not None
    assert step.actor is not None
    assert step.inbox is not None
    assert step.outbox is not None
    assert step.core is not None
    step_outbox:Mailbox = await store.load(step.outbox)
    assert len(step_outbox) == 1