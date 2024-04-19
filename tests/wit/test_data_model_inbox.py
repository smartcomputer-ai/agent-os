import os
from aos.grit import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit.data_model import *

def get_random_actor_id() -> ActorId:
    return get_object_id(os.urandom(20))

async def create_messages(store:ObjectStore, sender:ActorId, recipient:ActorId, count:int) -> list[MessageId]:
    message_ids = []
    previous_id = None
    for i in range(count):
        message_content_id = await store.store(Blob(None, bytes(f"message {i+1}", 'utf-8')))
        message = Message(previous_id, None, message_content_id)
        previous_id = await store.store(message)
        message_ids.append(previous_id)
    return message_ids

async def create_new_inbox(store:ObjectStore, actor:ActorId, senders:list[ActorId]) -> Mailbox:
    inbox = Mailbox()
    for sender in senders:
        message_ids = await create_messages(store, sender, actor, 5)
        inbox[sender] = message_ids[-1] #last message id is the head of the linked list
    return inbox

async def test_inbox_read_all():
    store = MemoryObjectStore()

    actor_id = get_random_actor_id()
    senders_ids = [get_random_actor_id(), get_random_actor_id(), get_random_actor_id()]

    new_inbox = await create_new_inbox(store, actor_id, senders_ids)
    inbox = Inbox(store, None, new_inbox)

    msgs = await inbox.read_new(1)
    #there are 3 senders, and we read one of each
    assert len(msgs) == 3
    for msg in msgs:
        msg_content = (await msg.get_content()).get_as_str()
        assert msg_content == "message 1"

    #persist the inbox, having read only one message from each sender
    read_inbox_id = await inbox.persist(store)

    #create a new inbox from the persisted id, and read all remaining messages
    #we should now get the second set of messages
    inbox = await Inbox.from_inbox_id(store, read_inbox_id, new_inbox)
    msgs = await inbox.read_new()
    assert len(msgs) == 4*3 # 4 messags remain from 3 senders
    for msg in msgs:
        msg_content = (await msg.get_content()).get_as_str()
        assert msg_content in ["message 2", "message 3", "message 4", "message 5"]
        assert msg_content != "message 1"

    #persist the inbox, having read all messags
    # and check that the persisted final inbox matches the in-memory view of "new_inbox"
    read_inbox_id = await inbox.persist(store)
    read_inbox = await store.load(read_inbox_id)
    assert len(read_inbox) == 3
    assert len(read_inbox) == len(new_inbox)
    assert read_inbox == new_inbox

    