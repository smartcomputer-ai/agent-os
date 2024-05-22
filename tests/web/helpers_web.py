from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from aos.wit import *
from aos.runtime.web import *
from aos.runtime.core import *

def setup_runtime() -> Runtime:
    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    runtime = Runtime(
        store=store, 
        references=refs,
        point=0,
        resolver=resolver)
    return runtime

def get_grit_url_prefix(runtime:Runtime) -> str:
    return f"/ag/{runtime.agent_id.hex()}/grit"

def get_wit_url_prefix(runtime:Runtime) -> str:
    return f"/ag/{runtime.agent_id.hex()}/wit"

async def create_object_from_content(runtime:Runtime, content:bytes|str|dict) -> ObjectId:
    b_obj = BlobObject.from_content(content)
    return await b_obj.persist(runtime.store)

async def create_object(runtime:Runtime, object:Object) -> ObjectId:
    return await runtime.store.store(object)

async def create_genesis_message(store:ObjectStore, sender_id:ActorId, wit_name:str, query_ref:str=None) -> MailboxUpdate:
    '''Creates a genesis message and returns a MailboxUpdate'''
    if(wit_name is None):
        raise Exception('wit_name must not be None')
    gen_core:TreeObject = Core.from_external_wit_ref(wit_name, query_ref)
    gen_core.maket("first").maket("second").makeb("third").set_as_str("made it")
    gen_message = await OutboxMessage.from_genesis(store, gen_core)
    gen_message_id = await gen_message.persist(store)
    return (sender_id, gen_message.recipient_id, gen_message_id)

async def create_and_send_genesis_message(runtime:Runtime, wit_ref:str, query_ref:str=None) -> tuple[ActorId, MessageId]:
    #inject a genesis message as a mailbox update so we have access to the new actor id
    sender_id, new_actor_id, gen_message_id = await create_genesis_message(runtime.store, runtime.agent_id, wit_ref, query_ref)
    await runtime.inject_mailbox_update((sender_id, new_actor_id, gen_message_id))
    return (new_actor_id, gen_message_id)

async def create_and_send_new_message(runtime:Runtime, recipient_id:ActorId, content:any) -> MessageId:
    msg = OutboxMessage.from_new(recipient_id, content)
    return await runtime.inject_message(msg)