from aos.wit import *
from aos.runtime.core import *
from aos.grit.stores.memory import MemoryObjectStore, MemoryReferences
from jetpack.coder.retriever_wit import *
from jetpack.coder.coder_wit import app as coder_app
from jetpack.messages import CodeSpec
from .helpers_runtime import *

# run with: poetry run pytest -s -o log_cli=true examples/coder/tests/

async def test_retrieve():

    store = MemoryObjectStore()
    refs = MemoryReferences()
    resolver = ExternalResolver(store)
    resolver.register('retriever_wit', app)
    resolver.register('coder_wit', coder_app)

    runtime = Runtime(store, refs, resolver=resolver)
    running_task = asyncio.create_task(runtime.start())

    spec = CodeSpec(
        task_description="""Get the data from this site (http://127.0.0.1:5001/ag/demodata/wit/actors/frontend_data/query/companies) and append it to csv file at '/home/lukas/test.csv'""",
        input_spec=json.loads('{"properties": {}, "type": "object"}'),
        output_spec=json.loads('{"properties": {"rows_updated": {"title": "How many rows were appended", "type": "string"}}, "required": ["rows_updated"], "title": "Output", "type": "object"}'),
        )
    
    #genesis of the generator actor
    create_actor_message = await create_retriever_actor(
        store, 
        spec,
        None,
        "retriever_wit")
    
    await runtime.inject_message(create_actor_message)
    await asyncio.sleep(0.1)

    message = await wait_for_message_type(runtime, "complete")
    # result:CodeExecuted = (await BlobObject.from_blob_id(runtime.store, message.content)).get_as_model(CodeExecuted)
    # print("execution output:", result.output)
    # assert result.output["id"] is not None
    
    print("stopping test runtime")
    #stop
    runtime.stop()
    await asyncio.wait_for(running_task, timeout=1) 