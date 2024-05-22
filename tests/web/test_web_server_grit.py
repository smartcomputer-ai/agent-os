import os
from starlette.testclient import TestClient
from aos.wit import *
from aos.runtime.web import *
from aos.runtime import *
import helpers_web as helpers

async def test_grit_get_refs_and_ref():
    runtime = helpers.setup_runtime()
    url_prefix = helpers.get_grit_url_prefix(runtime)

    runtime_task = asyncio.create_task(runtime.start())
    client = TestClient(WebServer(runtime).app())

    #refs should be empty at first
    response = client.get(url_prefix+"/refs")
    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/json'
    assert response.json() == {}

    #add some refs
    blob_1_id = await helpers.create_object_from_content(runtime, "blob_1")
    blob_2_id = await helpers.create_object_from_content(runtime, "blob_2")
    await runtime.references.set("ref_1", blob_1_id)
    await runtime.references.set("ref_2", blob_2_id)
    #refs should now return two refs
    response = client.get(url_prefix+"/refs")
    assert response.status_code == 200
    reponse_json = response.json()
    assert len(reponse_json) == 2
    assert reponse_json['ref_1'] == blob_1_id.hex()
    assert reponse_json['ref_2'] == blob_2_id.hex()

    #test the single ref endpoint
    response = client.get(url_prefix+"/refs/ref_1")
    reponse_json = response.json()
    assert response.status_code == 200
    assert len(reponse_json) == 1
    assert reponse_json['ref_1'] == blob_1_id.hex()

    runtime.stop()
    await runtime_task

async def test_grit_get_objects():
    runtime = helpers.setup_runtime()
    url_prefix = helpers.get_grit_url_prefix(runtime)

    runtime_task = asyncio.create_task(runtime.start())
    client = TestClient(WebServer(runtime).app())

    #test getting a non-existent object
    # with invalid id
    response = client.get(url_prefix+"/objects/abc")
    assert response.status_code == 400
     # with valid id, but not existing
    response = client.get(url_prefix+"/objects/"+to_object_id_str(get_object_id(os.urandom(20))))
    assert response.status_code == 404

    #add some objects
    blob_1_id = await helpers.create_object_from_content(runtime, "blob_1")
    blob_2_id = await helpers.create_object_from_content(runtime, b"blob_2")
    blob_3_id = await helpers.create_object_from_content(runtime, {"key1": "value1"})
    response = client.get(url_prefix+"/objects/"+to_object_id_str(blob_1_id))
    #should return as text
    assert response.status_code == 200
    assert response.headers['content-type'] == 'text/plain; charset=utf-8'
    assert response.text == "blob_1"
    #should return as binary
    response = client.get(url_prefix+"/objects/"+to_object_id_str(blob_2_id))
    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/octet-stream'
    assert response.content == b"blob_2"
    #should return as json
    response = client.get(url_prefix+"/objects/"+to_object_id_str(blob_3_id))
    assert response.status_code == 200
    assert response.headers['content-type'] == 'application/json'
    assert response.json() == {"key1": "value1"}

    runtime.stop()
    await runtime_task
