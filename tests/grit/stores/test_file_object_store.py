import os
from src.grit import *
from src.grit.stores.file import FileObjectStore

def get_random_object_id() -> ObjectId:
    return get_object_id(os.urandom(20))

async def test_read_write(tmp_path):
    
    objectstore = FileObjectStore(str(tmp_path))

    #save
    blob = Blob({'hi': 'there', 'foo': 'bar'}, os.urandom(1024))
    blob_id = await objectstore.store(blob)

    tree = {
        'a': get_random_object_id(), 
        'b': get_random_object_id(),
        'c': get_random_object_id()}
    tree_id = await objectstore.store(tree)

    message_log = Message(
        get_random_object_id(),
        None,
        get_random_object_id())
    message_log_id = await objectstore.store(message_log)

    mailbox = {
        get_random_object_id(): get_random_object_id(), 
        get_random_object_id(): get_random_object_id(),
        get_random_object_id(): get_random_object_id()}
    mailbox_id = await objectstore.store(mailbox)

    step = Step(
        get_random_object_id(),
        get_random_object_id(),
        get_random_object_id(),
        get_random_object_id(),
        get_random_object_id())
    step_id = await objectstore.store(step)

    #load
    blob_2 = await objectstore.load(blob_id)
    assert blob == blob_2

    tree_2 = await objectstore.load(tree_id)
    assert tree == tree_2

    message_log_2 = await objectstore.load(message_log_id)
    assert message_log == message_log_2

    mailbox_2 = await objectstore.load(mailbox_id)
    assert mailbox == mailbox_2

    step_2 = await objectstore.load(step_id)
    assert step == step_2