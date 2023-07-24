from starlette.testclient import TestClient
from src.wit import *
from src.web import *
from src.runtime import *
import helpers_web as helpers

async def test_runt_web_server_empty():
    runtime = helpers.setup_runtime()

    runtime_task = asyncio.create_task(runtime.start())
    client = TestClient(WebServer(runtime).app())

    response = client.get("/")
    runtime.stop()
    await runtime_task

    assert response.status_code == 200
    assert response.text == "Wit API"

async def test_get_agents():
    runtime = helpers.setup_runtime()

    runtime_task = asyncio.create_task(runtime.start())
    client = TestClient(WebServer(runtime).app())

    response = client.get("/ag")
    runtime.stop()
    await runtime_task

    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/json'
    # /agents should return a list of agent ids, since the runtime only supports a single agent, it can only be one id
    assert response.json() == {'test': runtime.agent_id.hex()}

