import os
from aos.grit import *
from aos.wit import *

def get_random_actor_id() -> ActorId:
    return get_object_id(os.urandom(20))

async def create_genesis_message(store:ObjectStore, sender_id:ActorId, wit_name:str) -> MailboxUpdate:
    '''Creates a genesis message and returns a MailboxUpdate'''
    gen_core:TreeObject = Core.from_external_wit_ref(store, wit_name)
    gen_core.maket('data').makeb('args').set_as_json({'hello': 'world'})
    gen_message = await OutboxMessage.from_genesis(store, gen_core)
    gen_message_id = await gen_message.persist(store)
    return (sender_id, gen_message.recipient_id, gen_message_id)

async def create_new_message(store:ObjectStore, sender_id:ActorId, recipient_id:ActorId, previous_message_id:MessageId|None, content:str|BlobObject|TreeObject) -> MailboxUpdate:
    '''Creates a new message and returns a MailboxUpdate'''
    if(isinstance(content, str)):
        content = BlobObject.from_str(content)
    content_id = await content.persist(store)
    message = Message(previous_message_id, None, content_id)
    message_id = await store.store(message)
    return (sender_id, recipient_id, message_id)

async def create_actor(store:ObjectStore, refs:References, sender_id:ActorId, wit_name:str):
    sender_id, new_actor_id, gen_message_id = await create_genesis_message(store, sender_id, wit_name)
    gen_mailbox = {sender_id: gen_message_id}
    gen_inbox_id = await store.store(gen_mailbox)
    first_step = Step(None, new_actor_id, gen_inbox_id, None, new_actor_id) #core_id is the same as actor_id
    first_step_id = await store.store(first_step)
    await refs.set(ref_step_head(new_actor_id), first_step_id)
    return first_step_id
