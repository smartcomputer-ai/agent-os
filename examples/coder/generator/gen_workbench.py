import types
from grit import *
from wit import *
# from .completions import SpecifyCode, normalize_prompt, strip_code
# from ..common import *

# upscale_spec = SpecifyCode(
#     task_description=normalize_prompt("""
#     Can you write me a module that loads an image and upscales it to 2000x2000 pixels while maintaining the 
#     aspect ratio? Then saves the image again.
#     The name of the image is passed as an id.
#     The upscale function should return the new id.
#     """),
# #     arguments_spec="""
# # class Input:
# #     id:str
# #     """,
# #     return_spec="""
# # class Output:
# #     id:str
# #     """,
#     )

# download_and_upscale_spec = SpecifyCode(
#     task_description=normalize_prompt("""
#     Can you download an image and upscales it to 2000x2000 pixels while maintaining the 
#     aspect ratio? Then saves the image again.
#     """),
# #     arguments_spec="""
# # class Input:
# #     img_url:str
# #     """,
# #     return_spec="""
# # class Output:
# #     id:str
# #     """,
#     )

# find_and_save = SpecifyCode(
#     task_description=normalize_prompt("""
#     Find an image of a cat online and save it.
#     """),
#     arguments_spec=None,
# #     return_spec="""
# # class Output:
# #     id:str
# #     """,
#     )

# top_stocks = SpecifyCode(
#     task_description=normalize_prompt("""
#     Find me a list of the top 10 stocks to buy when inflation is high and interest rates are high.
#     Save the list as a text file.
#     """),
#     arguments_spec=None,
# #     return_spec="""
# # class Output:
# #     id:str
# #     """,
#     )

def create_module(code:str):
    # Create a new module
    module_name = 'dynamic_module'
    dynamic_module = types.ModuleType(module_name)

    # Execute the string in the module's namespace
    exec(code, dynamic_module.__dict__)
    return dynamic_module

test_code = """
from PIL import Image
from io import BytesIO

async def upscale_image(image_id, store):
    # Load image bytes
    image_bytes = await store.load_bytes(image_id)

    # Open image
    image = Image.open(BytesIO(image_bytes))

    # Calculate aspect ratio
    aspect_ratio = image.width / image.height

    # Calculate new dimensions
    new_width = 2000
    new_height = int(new_width / aspect_ratio)

    # Resize image
    resized_image = image.resize((new_width, new_height))

    # Save image to bytes
    output_bytes = BytesIO()
    resized_image.save(output_bytes, format='PNG')
    output_bytes = output_bytes.getvalue()

    # Store new image
    new_image_id = await store.store_bytes(output_bytes)

    return new_image_id

async def entry(input, store):
    new_image_id = await upscale_image(input['id'], store)
    return {'id': new_image_id}
"""

test_code_2 = """
import requests
from PIL import Image
from io import BytesIO

async def download_image(url: str):
    response = requests.get(url)
    img = Image.open(BytesIO(response.content))
    return img

async def upscale_image(img, max_size=(2000, 2000)):
    img.thumbnail(max_size, Image.LANCZOS)
    return img

async def save_image(img, store):
    img_byte_arr = BytesIO()
    img.save(img_byte_arr, format='PNG')
    img_byte_arr = img_byte_arr.getvalue()
    id = await store.store_bytes(img_byte_arr)
    return id

async def entry(input, store):
    img_url = input['img_url']
    img = await download_image(img_url)
    img = await upscale_image(img)
    id = await save_image(img, store)
    return {"id": id}
"""

test_code_3 = """
 ```python
import requests
from PIL import Image
from io import BytesIO

async def entry(input, store):
    # Download the image
    response = requests.get(input['img_url'])
    img = Image.open(BytesIO(response.content))

    # Calculate the aspect ratio
    width, height = img.size
    aspect_ratio = width / height

    # Calculate the new dimensions while maintaining the aspect ratio
    if width > height:
        new_width = 2000
        new_height = int(new_width / aspect_ratio)
    else:
        new_height = 2000
        new_width = int(new_height * aspect_ratio)

    # Resize the image
    img = img.resize((new_width, new_height), Image.LANCZOS)

    # Save the image to a BytesIO object
    img_byte_arr = BytesIO()
    img.save(img_byte_arr, format='PNG')
    img_byte_arr = img_byte_arr.getvalue()

    # Store the image and get the id
    id = await store.store_bytes(img_byte_arr)

    return {"id": id}
```
The error "module 'PIL.Image' has no attribute 'ANTIALIAS'" occurs because the attribute 'ANTIALIAS' has been replaced with 'LANCZOS' in recent versions of PIL.

The error "cannot identify image file <_io.BytesIO object at 0x7f9ba15025c0>" occurs because the image data is not being correctly loaded into the BytesIO object. This can be fixed by ensuring that the image data is correctly loaded into the BytesIO object before trying to open it with PIL.
"""

class Input(BaseModel):
    img_url:str

class SpecifyCode(BaseModel):
    task_description: str
    arguments_spec: dict|None = None
    return_spec: dict|None = None
    max_code_tries:int|None = None
    test_descriptions: list[str]|None = None

async def amain():
    # code = await code_completion(top_stocks)
    # code = strip_code(code)
    # print(code)
    # print("executing code...")
    # mod = create_module(code)
    # store = StoreWrapper(MemoryObjectStore())
    # #input = {"img_url":"https://i.imgur.com/06lMSD5.jpeg"}
    # #output = await mod.entry(input, store=store)
    # output = await mod.entry(store=store)
    # print("output:", output)
    # print("saving output...")
    # id = output['id']
    # data = await store.load_bytes(id)
    # with open("output.txt", "wb") as f:
    #     f.write(data)
    # print("done")

    print("printing SpecifyCode schema")
    print(SpecifyCode.model_json_schema())

    # input_schema = Input(img_url="test").model_json_schema()
    # print("input schema:", json.dumps(input_schema))

    # print("testing resolver code...")
    # code_str = test_code_2
    # success, module_or_error = try_exec_module(code_str)
    # print("try_exec",success, module_or_error)

    # store = MemoryObjectStore()
    # storeWrapper = StoreWrapper(store)

    # core = Core(store, None, None)
    # core.makeb_path("code_test/generated.py").set_as_str(code_str)
    # core.makeb("wit_code_test").set_as_str("/code_test:generated:entry")
    # core_id = await core.persist()
    # print("will resolve")
    # resolver = CoreResolver(store)
    # func = await resolver.resolve(core_id, "wit_code_test", is_required=True)
    # print("func:", func)  
    # # asking to generate 
    # input = await function_completion(
    #     "entry", 
    #     download_and_upscale_spec.task_description, 
    #     input_schema, 
    #     "Please apply this to this image: https://i.imgur.com/06lMSD5.jpeg",
    # )
    # print("input:", input)
    # function_kwargs = {}
    # function_kwargs['input'] = input
    # function_kwargs['store'] = storeWrapper
    # output = await func(**function_kwargs)
    # print("output:", output)

def main():
    print("Testing gen wit code completion")
    asyncio.run(amain())

    print("Done")



