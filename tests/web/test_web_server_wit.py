from starlette.testclient import TestClient
from aos.wit import *
from aos.runtime.web import *
from aos.runtime.core import *
import helpers_web as helpers

#===================================================================================================
# Wits
#===================================================================================================
wit_a = Wit()
@wit_a.run_wit
async def wit_a_func(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
    print('wit_a')
    actor_id:ActorId = kwargs['actor_id']
    inbox_messages = await inbox.read_new()

    for msg in inbox_messages:
        if msg.content_id == actor_id:
            print("wit_a: got genesis message")
        else:
            print("wit_a: got a message")
            outbox.add(OutboxMessage.from_reply(msg, "hi back"))

#===================================================================================================
# Tests
#===================================================================================================
async def test_wit_get_actors():
    runtime = helpers.setup_runtime()
    runtime.resolver.register('wit_a', wit_a)
    url_prefix = helpers.get_wit_url_prefix(runtime)

    runtime_task = asyncio.create_task(runtime.start())
    client = TestClient(WebServer(runtime).app())
    await asyncio.sleep(0.02) #give time to start the runtime

    #there should be no actors right now
    response = client.get(url_prefix+"/actors")
    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/json'
    assert response.json() == []

    #create and actor for wit_a
    wit_a_actor_id, wit_a_gen_message_id = await helpers.create_and_send_genesis_message(runtime, 'wit_a')
    wit_a_actor_id_str = to_object_id_str(wit_a_actor_id)
    await asyncio.sleep(0.02) #give the runtime time to create the actor

    #there should be one actor now
    response = client.get(url_prefix+"/actors")
    assert response.status_code == 200
    assert response.json() == [wit_a_actor_id_str]

    runtime.stop()
    await runtime_task


async def test_wit_get_inbox_outbox():
    runtime = helpers.setup_runtime()
    runtime.resolver.register('wit_a', wit_a)
    url_prefix = helpers.get_wit_url_prefix(runtime)

    runtime_task = asyncio.create_task(runtime.start())
    client = TestClient(WebServer(runtime).app())

    #create and actor for wit_a
    wit_a_actor_id, wit_a_gen_message_id = await helpers.create_and_send_genesis_message(runtime, 'wit_a')
    wit_a_actor_id_str = to_object_id_str(wit_a_actor_id)
    await asyncio.sleep(0.02) #give the runtime time to create the actor

    #get the inbox for the wit_a actor
    # there should be a single genesis message in the inbox
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/inbox")
    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/json'
    expected_inbox = {runtime.agent_id.hex(): wit_a_gen_message_id.hex()} #the sender is the runtime, the message id is the genesis message id
    assert response.json() == expected_inbox

    #get the outbox for the wit_a actor
    # should be empty
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/outbox")
    assert response.status_code == 200
    assert response.json() == {}
    
    #send a normal message to the actor which will reply and so create an outbox message
    # the message is sent directly through the runtime, not the POST api (see test below for that)
    await helpers.create_and_send_new_message(runtime, wit_a_actor_id, "hi")
    await asyncio.sleep(0.2) #give the runtime time to process the message
    #check the outbox again
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/outbox")
    assert response.status_code == 200
    assert len(response.json()) == 1

    runtime.stop()
    await runtime_task

async def test_wit_post_inbox():
    runtime = helpers.setup_runtime()
    runtime.resolver.register('wit_a', wit_a)
    url_prefix = helpers.get_wit_url_prefix(runtime)

    runtime_task = asyncio.create_task(runtime.start())
    client = TestClient(WebServer(runtime).app())

    #create and actor for wit_a
    wit_a_actor_id, wit_a_gen_message_id = await helpers.create_and_send_genesis_message(runtime, 'wit_a')
    wit_a_actor_id_str = to_object_id_str(wit_a_actor_id)
    await asyncio.sleep(0.02) #give the runtime time to create the actor

    #get the inbox for the wit_a actor
    # there should be a single genesis message in the inbox
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/inbox")
    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/json'
    expected_inbox = {runtime.agent_id.hex(): wit_a_gen_message_id.hex()} #the sender is the runtime, the message id is the genesis message id
    assert response.json() == expected_inbox

    #send a message via POST api to the actor
    response = client.post(url_prefix+"/actors/"+wit_a_actor_id_str+"/inbox", json={"content":"hi"})
    assert response.status_code == 201
    assert response.headers['content-type'] == 'text/plain; charset=utf-8'
    new_message_id_str = response.text

    await asyncio.sleep(0.02) #give the runtime time to process the message
    #check the inbox again, it should now have moved to the new message
    response = client.get(url_prefix+"/actors/"+wit_a_actor_id_str+"/inbox")
    assert response.status_code == 200
    #the inbox is still len 1 because the sender fro both the genesis message and the posted message is the same
    assert len(response.json()) == 1
    assert response.json()[runtime.agent_id.hex()] == new_message_id_str
    #load the message and inspect it
    msg:Message = await runtime.store.load(to_object_id(new_message_id_str))
    assert msg is not None
    blob:Blob = await runtime.store.load(msg.content)
    assert 'ct' in blob.headers
    assert blob.headers['ct'] == 'j'
    assert json.loads(blob.data) == {"content":"hi"}

    runtime.stop()
    await runtime_task


