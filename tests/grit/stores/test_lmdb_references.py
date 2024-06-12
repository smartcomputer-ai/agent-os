import os
from aos.grit import *
from aos.grit.stores.lmdb import SharedEnvironment, LmdbReferences

def get_random_object_id() -> ObjectId:
    return get_object_id(os.urandom(20))

async def test_read_write(tmp_path):
    shared_env = SharedEnvironment(str(tmp_path))
    references = LmdbReferences(shared_env)
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

    all_ids = await references.get_all()
    assert len(all_ids) == 4

async def test_read_after_close(tmp_path):
    shared_env = SharedEnvironment(str(tmp_path))
    references = LmdbReferences(shared_env)

    tree_id = get_random_object_id()
    await references.set('tree', tree_id)
    shared_env.get_env().close() 
    del shared_env
    del references

    shared_env = SharedEnvironment(str(tmp_path))
    references = LmdbReferences(shared_env)
    tree_id_2 = await references.get('tree')
    assert tree_id == tree_id_2