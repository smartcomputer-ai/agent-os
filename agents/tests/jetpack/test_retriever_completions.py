from jetpack.coder.retriever_completions import *
from jetpack.messages.messages_coder import CodeSpec

async def test_coder_retrieval__none_needed():
    spec = CodeSpec(
        task_description="""Download an image from a provided URL, and upscales the image to 2000x2000 pixels while maintaining the aspect ratio? Use Image.LANCZOS. Then save the image again and return the id of the persisted image.""",
        input_spec=json.loads('{"properties": {"img_url": {"title": "Img Url", "type": "string"}}, "required": ["img_url"], "title": "Input", "type": "object"}'),
        output_spec=json.loads('{"properties": {"id": {"title": "Store Id", "type": "string"}}, "required": ["id"], "title": "Output", "type": "object"}'),
        input_examples=[
            "Use the following image: https://i.imgur.com/06lMSD5.jpeg",
        ],)
    result = await retrieve_completion(
        spec.task_description,
        spec.input_examples,
    )
    assert result is None


async def test_coder_retrieval__two_needed():
    spec = CodeSpec(
        task_description="""Get the data from this site (http://127.0.0.1:5001/ag/demodata/wit/actors/frontend_data/query/companies) and append it to excel sheet at /home/me/report.xlsx""",
        input_spec=json.loads('{"properties": {}, "type": "object"}'),
        output_spec=json.loads('{"properties": {"rows_updated": {"title": "How many rows were appended", "type": "string"}}, "required": ["rows_updated"], "title": "Output", "type": "object"}'),
        )
    result = await retrieve_completion(
        spec.task_description,
        spec.input_examples,
    )
    assert result is not None
    assert len(result) == 2
    assert "http://127.0.0.1:5001/ag/demodata/wit/actors/frontend_data/query/companies" in result
    assert "/home/me/report.xlsx" in result