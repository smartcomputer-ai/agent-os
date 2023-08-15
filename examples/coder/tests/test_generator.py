from wit import *
from runtime import *
from grit.stores.memory import MemoryObjectStore, MemoryReferences
from examples.coder.generator import app, SpecifyCode, normalize_prompt

# run with: poetry run pytest -s -o log_cli=true examples/coder/tests/

async def create_genesis_message(store:ObjectStore, sender_id:ActorId, wit_name:str):
    '''Creates a genesis message and returns a MailboxUpdate'''
    gen_core:TreeObject = Core.from_external_wit_ref(store, wit_name)
    #gen_core.maket('data').makeb('args').set_as_json({'hello': 'world'})
    gen_message = await OutboxMessage.from_genesis(store, gen_core)
    gen_message_id = await gen_message.persist(store)
    return (sender_id, gen_message.recipient_id, gen_message_id)

async def test_generate():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('gen_wit', app)

    runtime = Runtime(store, refs, "test", resolver)
    running_task = asyncio.create_task(runtime.start())

    #genesis of the generator actor
    create_actor_message = await create_genesis_message(store, runtime.agent_id, 'gen_wit')
    await runtime.inject_mailbox_update(create_actor_message)
    await asyncio.sleep(0.2)

    #send the spec of what should be coded
    upscale_spec = SpecifyCode(
        task_description=normalize_prompt("""
        Can you download an image and upscales it to 2000x2000 pixels while maintaining the aspect ratio? 
        Then saves the image again.
        """),
        arguments_spec=json.loads('{"properties": {"img_url": {"title": "Img Url", "type": "string"}}, "required": ["img_url"], "title": "Input", "type": "object"}'),
        return_spec=json.loads('{"properties": {"id": {"title": "Store Id", "type": "string"}}, "required": ["id"], "title": "Output", "type": "object"}'),
        test_descriptions=[
            "Use the following image: https://i.imgur.com/06lMSD5.jpeg",
            "Can you try this https://i.imgur.com/E0IOEPx.jpeg"
        ],)
    await runtime.inject_message(OutboxMessage.from_new(create_actor_message[1], upscale_spec, mt="spec"))

    #wait for the whole thing to run
    await asyncio.sleep(300)

    #stop
    runtime.stop()
    await asyncio.wait_for(running_task, timeout=1) 