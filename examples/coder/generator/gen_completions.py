import os
import openai
import re
from dotenv import load_dotenv
from grit import *
from wit import *
from common import *

#========================================================================
# Utils
#========================================================================
def normalize_prompt(prompt:str)->str:
    return " ".join(line.strip() for line in prompt.splitlines())

def strip_code(code:str) -> str:
    #if the code is on within a markdown code block, extract it
    #remove all linest start with ```
    if "```" in code:
        if "```python" or "```Python" in code:
            regex = r"```[pP]ython\n([\s\S]*?)```"
        else:
            regex = r"```([\s\S]*?)```"
        match = re.search(regex, code)
        if match:
            code = match.group(1)
            return code
        else:
            raise Exception("Could not find code in markdown code block")
    else:
        return code
    
#========================================================================
# Prompts
#========================================================================
store_spec = """All data is stored in a simple key-value store that is accessed with simple string IDs. When persisting data, the store generates the ids. The inteface of the store is:
```
class StoreWrapper:
    async def load_bytes(self, id:str) -> bytes | None:
        pass
    async def load_str(self, id:str) -> str | None:
        pass
    async def load_json(self, id:str) -> dict | None:
        pass
    async def store_bytes(self, data:bytes) -> str | None:
        pass
    async def store_str(self, data:str) -> str | None:
        pass
    async def store_json(self, data:dict) -> str | None:
        pass
```
""" + normalize_prompt("""
An instance of the store can be passed to your the function you write. For example: `async def entry(..., store:StoreWrapper)`. 
The StoreWrapper type can be imported with `from .common import *`, but do not import it and do not type annotate it.
Many times the input spec, if there is one, will contain ids to define the content that needs to be loaded. Use the store to load those ids.
Many times the output spec, if there is one, will contain fields to return changed data. Use the store to store those fields and populate the ids in the return values.
""")

module_spec = normalize_prompt("""
The functions in the module can have side effects. So, in the code we write, you can call you to external APIs, databases, file system and so on.
Whatever it takes to accomplish the task defined below.
You can import any module in the standard library, for example: `import os` or `from datetime import datetime`.                            
However, only the following commonly-known third-party modules and their submodules may be imported:
""")                      
third_party_modules = [
"pydantic", 
"jinja2", 
"openai", 
"pandas", 
"requests", 
"PIL",
"scipy",
"numpy",
"sklearn",
]
module_spec += ", ".join(third_party_modules)

#========================================================================
# OpenAI GPT Completions
#========================================================================
load_dotenv()
openai.api_key = os.getenv("OPENAI_API_KEY")

async def code_completion(
        task_description:str, 
        plan:str|None=None, 
        arguments_spec:dict|None=None, 
        return_spec:dict|None=None, 
        previous_code:str|None=None, 
        previous_code_errors:str|None=None,
        ) -> str:
    
    system_message = normalize_prompt("""
    You are an expert coder and you assist with generating Python code. 
    Please write code for the requested feature, and only the 
    code in question with no premble or explanation. The code is executed inside a sandboxed environment.
    Write an entire python module with imports, etc. You can write more than one function.
    But there must be an entry point function, called 'entry', that follows the requested 
    arguments and return type.""")

    messages_completion = []
    messages_completion.append({
        'role': 'system',
        'content': system_message
    })
    for msg in [store_spec, module_spec]:
        messages_completion.append({
            'role': 'user',
            'content': msg
        })

    messages_completion.append({
        'role': 'user',
        'content': "Here is the task that the module needs to accomplish: " + task_description
    })

    if plan is not None:
        messages_completion.append({
            'role': 'user',
            'content': "Here is the plan on how to accomplish this:\n" + plan
        })

    input_output_spec = "Let's describe the desired input and output of the entry function."
    if arguments_spec:
        input_output_spec += f"The entry function needs to take an argument called `input` and it is of the following structure, treat it as a dict:\n"
        input_output_spec += f"```\n{json.dumps(arguments_spec)}\n```"
    else:
        input_output_spec += "The entry function does not take any arguments besides the store."

    if return_spec:
        input_output_spec += f"The entry function needs return an object of the following structure, return it as a dict:\n"
        input_output_spec += f"```\n{json.dumps(return_spec)}\n```"
    else:
        input_output_spec += "The entry function should not return a value, try to accomplish the task entirely with side-effects."
    messages_completion.append({
        'role': 'user',
        'content': input_output_spec
    })

    if previous_code is not None and previous_code_errors is not None:
        previous_code_spec = "Here is the code from previous attempts:\n"
        previous_code_spec += f"```\n{previous_code}\n```"
        previous_code_spec += "Here are the errors from that code:\n"
        previous_code_spec += previous_code_errors
        messages_completion.append({
            'role': 'user',
            'content': previous_code_spec
        })

    messages_completion.append({
            'role': 'user',
            'content': "If you must explain something, please do so in a comment. And always wrap the Python code in a code block like so:\n```python\n#your code here\n```"
        })

    response = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        messages=messages_completion,
        temperature=0.3)
    content = response['choices'][0]['message']['content']
    code = strip_code(content)
    return code


async def function_completion(
        function_name:str, 
        function_description:str, 
        parameters:dict, 
        input_description:str|None=None,
        ) -> dict:
    
    system_message = normalize_prompt("""
    You are an expert coder and you assist with generating Python code. 
    You generated the following function, and now we create a function call for it.""")

    messages_completion = []
    messages_completion.append({
        'role': 'system',
        'content': system_message
    })
    if input_description:
        messages_completion.append({
            'role': 'user',
            'content': "The data and type of input to generate for the function call is as follows: "+input_description
        })

    response = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        messages=messages_completion,
        temperature=0.3,
        functions=[
            {
                "name": function_name,
                "description": function_description,
                "parameters": parameters
            }
        ],
        function_call={ "name": function_name},
        )
    
    content = response['choices'][0]['message']['content']
    function_call = response['choices'][0]['message']['function_call']
    if function_call is None or function_call['name'] != function_name:
        raise Exception("Could not generate function call")
    arguments = function_call['arguments']
    #parse the arguments as JSON
    function_input = json.loads(arguments)
    return function_input

