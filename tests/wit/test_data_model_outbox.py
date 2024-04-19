import os
from aos.grit import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit.data_model import *

def get_random_actor_id() -> ActorId:
    return get_object_id(os.urandom(20))

async def get_message_content(store:ObjectStore, message_id:MessageId) -> str:
    message = await store.load(message_id)
    content_blob = await BlobObject.from_blob_id(store, message.content)
    return content_blob.get_as_str()

async def test_outbox_from_new():
    store = MemoryObjectStore()

    actor_id = get_random_actor_id()
    recipient_ids = [get_random_actor_id(), get_random_actor_id(), get_random_actor_id()]

    outbox = Outbox(None)
    outbox.add(OutboxMessage.from_new(recipient_ids[0], "message 1"))
    outbox.add(OutboxMessage.from_new(recipient_ids[1], "message 1"))
    outbox.add(OutboxMessage.from_new(recipient_ids[2], "message 1"))
    outbox_id = await outbox.persist(store)

    outbox_mailbox = await store.load(outbox_id)
    assert len(outbox_mailbox) == 3
    assert recipient_ids[0] in outbox_mailbox
    assert recipient_ids[1] in outbox_mailbox
    assert recipient_ids[2] in outbox_mailbox

    assert (await get_message_content(store, outbox_mailbox[recipient_ids[0]])) == "message 1"
    assert (await get_message_content(store, outbox_mailbox[recipient_ids[1]])) == "message 1"
    assert (await get_message_content(store, outbox_mailbox[recipient_ids[2]])) == "message 1"


async def test_outbox_from_previous():
    store = MemoryObjectStore()
    recipient_ids = [get_random_actor_id(), get_random_actor_id(), get_random_actor_id()]

    #create an outbox with 2 messages for each recipient
    outbox = Outbox(None)
    outbox.add(OutboxMessage.from_new(recipient_ids[0], "message 1"))
    outbox.add(OutboxMessage.from_new(recipient_ids[0], "message 2"))
    outbox.add(OutboxMessage.from_new(recipient_ids[1], "message 1"))
    outbox.add(OutboxMessage.from_new(recipient_ids[1], "message 2"))
    outbox.add(OutboxMessage.from_new(recipient_ids[2], "message 1"))
    outbox.add(OutboxMessage.from_new(recipient_ids[2], "message 2"))
    first_outbox_id = await outbox.persist(store)

    #create a new outbox from the previous one
    outbox = await Outbox.from_outbox_id(store, first_outbox_id)
    #add 1 more message for two of the recipients
    outbox.add(OutboxMessage.from_new(recipient_ids[0], "message 3"))
    outbox.add(OutboxMessage.from_new(recipient_ids[1], "message 3"))
    second_outbox_id = await outbox.persist(store)

    outbox_mailbox = await store.load(second_outbox_id)
    assert len(outbox_mailbox) == 3
    assert recipient_ids[0] in outbox_mailbox
    assert recipient_ids[1] in outbox_mailbox
    assert recipient_ids[2] in outbox_mailbox

    assert (await get_message_content(store, outbox_mailbox[recipient_ids[0]])) == "message 3"
    assert (await get_message_content(store, outbox_mailbox[recipient_ids[1]])) == "message 3"
    assert (await get_message_content(store, outbox_mailbox[recipient_ids[2]])) == "message 2" #did not add another message for this recipient
    


    