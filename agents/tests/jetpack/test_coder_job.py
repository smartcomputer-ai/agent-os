from aos.wit import *
from aos.runtime.core import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from jetpack.messages import CodeSpec, CodeExecution, CodeExecuted
from jetpack.coder.coder_wit import *
from .helpers_runtime import *

# run with: poetry run pytest -s -o log_cli=true agents/tests/

async def test_coder__img_resize_as_job():
    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('coder_wit', app)

    runtime = Runtime(store, refs, "test", resolver)
    running_task = asyncio.create_task(runtime.start())

    upscale_spec = CodeSpec(
        task_description="""Download an image from a provided URL, and upscales the image to 2000x2000 pixels while maintaining the aspect ratio? Use Image.LANCZOS. Then save the image again and return the id of the persisted image.""",
        input_spec=json.loads('{"properties": {"img_url": {"title": "Img Url", "type": "string"}}, "required": ["img_url"], "title": "Input", "type": "object"}'),
        output_spec=json.loads('{"properties": {"id": {"title": "Store Id", "type": "string"}}, "required": ["id"], "title": "Output", "type": "object"}'),
        input_examples=[
            "Use the following image: https://i.imgur.com/06lMSD5.jpeg",
        ],)
    
    job_exec = CodeExecution(
        input_arguments={"img_url": "https://i.imgur.com/E0IOEPx.jpeg"},
    )

    #genesis of the generator actor
    create_actor_message = await create_coder_actor(
        store, 
        "test_coder_job",
        upscale_spec,
        job_exec,
        "coder_wit")
    
    await runtime.inject_message(create_actor_message)
    await asyncio.sleep(0.1)

    message = await wait_for_message_type(runtime, "code_executed")
    result:CodeExecuted = (await BlobObject.from_blob_id(runtime.store, message.content)).get_as_model(CodeExecuted)
    print("execution output:", result.output)
    assert result.output["id"] is not None
    
    print("stopping test runtime")
    #stop
    runtime.stop()
    await asyncio.wait_for(running_task, timeout=1) 