import json
import re
from .function_builder import FunctionBuilder
from .prompt_builder import PromptBuilder

def build_chat_kwargs(
        prompts:PromptBuilder, 
        functions:FunctionBuilder|None=None, 
        function_call:str|None=None,
        ) -> dict:
    kwargs = {}
    if prompts is not None:
        kwargs['messages'] = prompts.build()
    if functions is not None:
        kwargs['functions'] = functions.build()
    if function_call is not None:
        kwargs['function_call'] = { "name": function_call }
    return kwargs

def parse_completions(response:dict) -> list[str|tuple[str, dict]]:
    completions = []
    for choice in response['choices']:
        if(choice['finish_reason'] == 'stop' or choice['finish_reason'] == 'function_call'):
            function_call = choice['message'].get('function_call')
            if function_call is not None:
                function_name = function_call['name']
                arguments_str = function_call['arguments']
                arguments:dict = json.loads(arguments_str)
                completions.append((function_name, arguments))
            else:
                completions.append(choice['message'].get('content'))
        else:
            raise Exception(f"Unexpected finish_reason: {choice['finish_reason']}")
    return completions

def parse_completion(response:dict) -> str|tuple[str, dict]:
    completions = parse_completions(response)
    if len(completions) == 1:
        return completions[0]
    else:
        raise Exception(f"Expected only one completion, but got {len(completions)}")

def parse_message_completion(response:dict) -> str:
    completion = parse_completion(response)
    if isinstance(completion, str):
        return completion
    else:
        raise Exception(f"Expected a message completion, not a function call, got: {completion}")

def parse_function_completion(response:dict) -> tuple[str, dict]:
    completion = parse_completion(response)
    if isinstance(completion, tuple):
        return completion
    else:
        raise Exception(f"Expected a function call completion, not a message, got: {completion}")

def parse_code_completion(response:dict) -> str:
    completions = parse_completions(response)
    if len(completions) == 1:
        if isinstance(completions[0], str):
            return strip_code(completions[0])
        else:
            raise Exception(f"Expected a message completion, not a function call, got: {completions[0]}")
    else:
        raise Exception(f"Expected only one completion, but got {len(completions)}")

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