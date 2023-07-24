import httpx
from httpx_sse import aconnect_sse
from starlette.testclient import TestClient
from src.wit import *
from src.web import *
from src.runtime import *
import helpers_web as helpers

#===================================================================================================
# Wits
#===================================================================================================
wit_a = Wit()
@wit_a.run_wit
async def wit_a_func(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
    #print('wit_a')
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
async def test_sse():
    runtime = helpers.setup_runtime()
    runtime.resolver.register('wit_a', wit_a)
    url_prefix = helpers.get_wit_url_prefix(runtime)

    runtime_task = asyncio.create_task(runtime.start())
    client = TestClient(WebServer(runtime).app())
    await asyncio.sleep(0.02) #give the runtime time to create the actor

    sse_events = []
    async def listen_to_messages():
        async with httpx.AsyncClient(app=WebServer(runtime).app(), base_url="http://localhost:5000") as client:
            async with aconnect_sse(client, method="GET", url=f"{url_prefix}/messages-sse?actor-id=all") as event_source:
                async for sse in event_source.aiter_sse():
                    print(f"SSE event (id: {sse.id}, event: {sse.event}): {sse.data}")
                    sse_events.append(sse.json())
                print("SSE connection closed")

    listen_task = asyncio.create_task(listen_to_messages())

    #create and actor for wit_a
    wit_a_actor_id, wit_a_gen_message_id = await helpers.create_and_send_genesis_message(runtime, 'wit_a')
    wit_a_actor_id_str = to_object_id_str(wit_a_actor_id)
    await asyncio.sleep(0.02) #give the runtime time to create the actor

    #send a message via POST api to the actor
    response = client.post(url_prefix+"/actors/"+wit_a_actor_id_str+"/inbox", json={"content":"hi"})
    assert response.status_code == 201
    assert response.headers['content-type'] == 'text/plain; charset=utf-8'
    new_message_id_str = response.text
    await asyncio.sleep(0.02) #give the runtime time to create the actor
    
    runtime.stop()
    await runtime_task
    await listen_task

    #there should be three events, two for the sent message (genesis and post), and one for the reply from the wit
    assert len(sse_events) == 3
    assert sse_events[0]['message_id'] == wit_a_gen_message_id.hex()
    assert sse_events[1]['message_id'] == new_message_id_str

    


