import os

from src.runtime.actor_executor import MailboxUpdate
from src.grit import *
from src.wit import *

def get_random_actor_id() -> ActorId:
    return get_object_id(os.urandom(20))

async def create_genesis_message(store:ObjectStore, sender_id:ActorId, wit_ref:str, query_ref:str=None) -> MailboxUpdate:
    '''Creates a genesis message and returns a MailboxUpdate'''
    gen_core:TreeObject = Core.from_external_wit_ref(store, wit_ref, query_ref)
    gen_core.maket('data').makeb('args').set_as_json({'hello': 'world'})
    gen_message = await OutboxMessage.from_genesis(store, gen_core)
    gen_message_id = await gen_message.persist(store)
    return (sender_id, gen_message.recipient_id, gen_message_id)

async def create_new_message(store:ObjectStore, sender_id:ActorId, recipient_id:ActorId, previous_message_id:MessageId|None, content:str|BlobObject|TreeObject, mt:str=None) -> MailboxUpdate:
    '''Creates a new message and returns a MailboxUpdate'''
    if(isinstance(content, str)):
        content = BlobObject.from_str(content)
    content_id = await content.persist(store)
    headers = None
    if(mt is not None):
        headers = {'mt': mt}
    message = Message(previous_message_id, headers, content_id)
    message_id = await store.store(message)
    return (sender_id, recipient_id, message_id)

async def setup_wit_with_dependencies(store:ObjectStore, wit_ref:str) -> tuple[dict, StepId|None, Mailbox]:
    (sender_id, recipient_id, gen_message_id) = await create_genesis_message(store, get_random_actor_id(), wit_ref)
    kwargs = {
        'agent_id': get_random_actor_id(),
        'actor_id': recipient_id,
        'object_store': store,
    }
    mailbox = {sender_id: gen_message_id}
    return (kwargs, None, mailbox) #none becuase it is the genesis step

async def setup_wit_prototype_with_dependencies(store:ObjectStore, wit_ref:str) -> tuple[dict, StepId|None, Mailbox]:
    (sender_id, recipient_id, gen_message_id) = await create_genesis_message(store, get_random_actor_id(), wit_ref)
    kwargs = {
        'agent_id': get_random_actor_id(),
        'actor_id': recipient_id,
        'object_store': store,
    }
    mailbox = {sender_id: gen_message_id}
    return (kwargs, None, mailbox) #none becuase it is the genesis step

async def setup_query_with_dependencies(store:ObjectStore, wit_function, wit_ref:str, query_ref:str) -> tuple[dict, StepId]:
    (sender_id, recipient_id, gen_message_id) = await create_genesis_message(store, get_random_actor_id(), wit_ref, query_ref)
    kwargs = {
        'agent_id': get_random_actor_id(),
        'actor_id': recipient_id,
        'object_store': store,
    }
    mailbox = {sender_id: gen_message_id}
    #run the wit function, to have an inital step
    new_step_id = await wit_function(*(None, mailbox), **kwargs)
    return (kwargs, new_step_id) 