from wit import *
from runtime import *
from grit.stores.memory import MemoryObjectStore, MemoryReferences
from jetpack.messages import CodeSpec
from jetpack.coder.coder_wit import app, create_coder_actor
from .helpers_runtime import *

# run with: poetry run pytest -s -o log_cli=true examples/coder/tests/

async def test_coder__img_resize():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('gen_wit', app)

    runtime = Runtime(store, refs, "test", resolver)
    running_task = asyncio.create_task(runtime.start())

    #genesis of the generator actor
    genesis_message = await create_coder_actor(
        store, "image_coder",
        None,
        None,
        'gen_wit')
    
    await runtime.inject_message(genesis_message)
    await asyncio.sleep(0.2)

    #send the spec of what should be coded
    upscale_spec = CodeSpec(
        task_description="""
        Can you download an image and upscales it to 2000x2000 pixels while maintaining the aspect ratio? 
        Then saves the image again.""",
        input_examples=[
            "Use the following image: https://i.imgur.com/06lMSD5.jpeg",
            "Can you try this https://i.imgur.com/E0IOEPx.jpeg"
        ],)
    await runtime.inject_message(OutboxMessage.from_new(genesis_message.recipient_id, upscale_spec, mt="spec"))

    message = await wait_for_message_type(runtime, "code_deployed")
    #todo, load image id and see what is in it

    print("stopping test runtime")
    #stop
    runtime.stop()
    await asyncio.wait_for(running_task, timeout=1) 