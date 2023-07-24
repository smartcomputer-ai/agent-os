import os
from src.grit import *
from src.grit.stores.file import FileReferences

def get_random_object_id() -> ObjectId:
    return get_object_id(os.urandom(20))

async def test_read_write(tmp_path):
    references = FileReferences(str(tmp_path))
    #save
    tree_id = get_random_object_id()
    await references.set('tree', tree_id)

    message_log_id = get_random_object_id()
    await references.set('message_log', message_log_id)

    mailbox_id = get_random_object_id()
    await references.set('mailbox', mailbox_id)

    step_id = get_random_object_id()
    await references.set('step', step_id)

    #load
    tree_id_2 = await references.get('tree')
    assert tree_id == tree_id_2

    message_log_id_2 = await references.get('message_log')
    assert message_log_id == message_log_id_2

    mailbox_id_2 = await references.get('mailbox')
    assert mailbox_id == mailbox_id_2

    step_id_2 = await references.get('step')
    assert step_id == step_id_2

    assert len(await references.get_all()) == 4

async def test_read_after_close(tmp_path):
    references = FileReferences(tmp_path)
    tree_id = get_random_object_id()
    other_id = get_random_object_id()
    await references.set('tree', tree_id)
    await references.set('a/b/c', other_id)
    del references

    references = FileReferences(tmp_path)
    tree_id_2 = await references.get('tree')
    other_id_2 = await references.get('a/b/c')
    assert tree_id == tree_id_2
    assert other_id == other_id_2
    assert len(await references.get_all()) == 2