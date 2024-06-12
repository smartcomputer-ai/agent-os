from starlette.testclient import TestClient
from aos.wit import *
from aos.runtime.web import *
from aos.runtime.core import *
import helpers_web as helpers

#===================================================================================================
# Wits
#===================================================================================================
wit_a = Wit(generate_wut_query=False)
@wit_a.run_wit
async def wit_a_func(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
    print('wit_a')
    inbox_messages = await inbox.read_new()

@wit_a.run_query
async def wit_a_query(query_name, core:Core) -> Blob:
    if(query_name == 'get_string'):
        return "Hello World"
    elif(query_name == "get_core"):
        return core
    elif(query_name == "get_html"):
        return Blob({'Content-Type': 'text/html'}, b"<html><body><h1>Hello World</h1></body></html>")

#===================================================================================================
# Tests
#===================================================================================================

async def setup_wit_and_query():
    runtime = helpers.setup_runtime()
    runtime.resolver.register('wit_a', wit_a)
    runtime.resolver.register('wit_a_query', wit_a)
    runtime_task = asyncio.create_task(runtime.start())
    #create and actor for wit_a
    wit_a_actor_id, wit_a_gen_message_id = await helpers.create_and_send_genesis_message(runtime, 'wit_a', 'wit_a_query')
    wit_a_actor_id_str = to_object_id_str(wit_a_actor_id)
    await asyncio.sleep(0.02) #give the runtime time to create the actor
    return (runtime, runtime_task, wit_a_actor_id_str, helpers.get_wit_url_prefix(runtime))

async def test_wit_query_get_string():
    runtime, runtime_task, wit_a_actor_id_str, url_prefix = await setup_wit_and_query()
    client = TestClient(WebServer(runtime).app())

    query_name = 'get_string'
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/query/"+query_name)
    assert response.status_code == 200
    assert response.text == "Hello World"

    runtime.stop()
    await runtime_task

async def test_wit_query_get_core():
    runtime, runtime_task, wit_a_actor_id_str, url_prefix = await setup_wit_and_query()
    client = TestClient(WebServer(runtime).app())

    query_name = 'get_core'
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/query/"+query_name)
    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/json'
    json_tree = response.json()
    assert len(json_tree) == 3

    #expect the core to have /first/second/third -> "made it"
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/query/"+query_name+"/first/")
    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/json'
    json_tree = response.json()
    assert len(json_tree) == 1
    
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/query/"+query_name+"/first/second/third")
    assert response.status_code == 200
    assert response.headers['content-type'] == 'text/plain; charset=utf-8'
    assert response.text == "made it"

    runtime.stop()
    await runtime_task

async def test_wit_query_get_html():
    runtime, runtime_task, wit_a_actor_id_str, url_prefix = await setup_wit_and_query()
    client = TestClient(WebServer(runtime).app())

    query_name = 'get_html'
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/query/"+query_name)
    assert response.status_code == 200
    assert response.headers['content-type'] == 'text/html; charset=utf-8'
    assert response.text == "<html><body><h1>Hello World</h1></body></html>"

    runtime.stop()
    await runtime_task

