import json
import os
import openai
from dotenv import load_dotenv
from openai.openai_object import OpenAIObject
from grit import *
from common import ChatMessage, SpecifyCode, ExecuteCode

#========================================================================
# Utils
#========================================================================
def normalize_prompt(prompt:str)->str:
    return " ".join(line.strip() for line in prompt.splitlines())


#========================================================================
# OpenAI GPT Completions
#========================================================================
load_dotenv()
openai.api_key = os.getenv("OPENAI_API_KEY")

async def code_spec_completion(
        messages:list[ChatMessage], 
        actor_id:ActorId|None=None,
        ) -> tuple[ChatMessage, SpecifyCode | None]:
    
    if(openai.api_key is None):
        return ChatMessage.from_actor(
            "I am a helpful assistant, and I want to reply to your question, but the OpenAI API key is not set", 
            actor_id), None

    system_message = normalize_prompt("""
        You are a helpful assistant. 
        Our goal is to write some Python code and flesh out the specifications of what that code should do.
        Use to code gen function when you believe you have enough information to summarize the desired functionality and pass it as specification.
        As long as you are not certain, ask clarifying questions.
                                        
        Also, define the input and output structures of the desired function using JSON schemas. 
        The generated function should take all inputs as a single object, not as several parameters or agruments, and return a single object as output.
        The new code can understand all kinds of input in string or other basic formats, for example to download an image, it could just take a URL as input.
                                      
        Also, this system uses a simple key-value store that is accessed with simple string IDs. Which, if they contain, media, can be rendered in the UI.
        So, if the function is changing data, retrieving data, or generating content, ask it to save the data in the data store, and then specify that it just 
        returns the id of the stored data and not the data itself.
        For example: instead of {"image_url": "https://example.com/image1.jpg"} return {"id": "<image_id>"} 
                                        
        Finally, since we are generating code, it is also necessary to have an example of how to call the function. 
        For example, if the function is called "download_image", then you should ask the user to give an example of a real URL to use for testing.
        """)
    messages_completion = []
    messages_completion.append({
        'role': 'system',
        'content': system_message
    })
    for msg in messages:
        messages_completion.append({
            'role': msg.from_name,
            'content': msg.content
        })
    response:OpenAIObject = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        messages=messages_completion,
        temperature=0.7,
        functions=[
            {
                "name": "code_gen",
                "description": normalize_prompt("""
                    This function generates code according to the given specification (task_description).
                    If the specification warrants inputs, provide an arguments_spec in JSON schema format.
                    If the specification warrants outputs, provide a return_spec in JSON schema format.
                    If the specification warrants tests, provide a list of test_descriptions, but often one test is enough. 
                    If you are not sure about how to test, ask the user for test data.
                    """),
                "parameters": {'properties': 
                    {
                        'task_description': {'title': 'Task Description', 'type': 'string'}, 
                        'arguments_spec': {'anyOf': [{'type': 'object'}, {'type': 'null'}], 'default': None, 'title': 'Arguments Spec'}, 
                        'return_spec': {'anyOf': [{'type': 'object'}, {'type': 'null'}], 'default': None, 'title': 'Return Spec'}, 
                        'test_descriptions': {'anyOf': [{'items': {'type': 'string'}, 'type': 'array'}, {'type': 'null'}], 'default': None, 'title': 'Test Descriptions'}
                    }, 
                    'required': ['task_description'], 
                    'title': 'SpecifyCode', 
                    'type': 'object'
                }
            }
        ],
    )

    code_spec = None
    if 'function_call' in response['choices'][0]['message']:
        function_call = response['choices'][0]['message']['function_call']
        function_args_str = function_call['arguments']
        function_args = json.loads(function_args_str)
        
        task_description = function_args.get('task_description')
        arguments_spec = function_args.get('arguments_spec')
        return_spec = function_args.get('return_spec')
        test_descriptions = function_args.get('test_descriptions')
        #sometimes, tests descriptions are not strings, convert them to strings
        if test_descriptions is not None:
            test_descriptions = [str(test_description) for test_description in test_descriptions]
        print('task_description', task_description)
        print('arguments_spec', arguments_spec)
        print('return_spec', return_spec)
        print('test_descriptions', test_descriptions)
        code_spec = SpecifyCode(
            task_description=task_description, 
            arguments_spec=arguments_spec, 
            return_spec=return_spec, 
            test_descriptions=test_descriptions)
            
    content = response['choices'][0]['message'].get('content')
    if content is None and code_spec is not None:
        content = "I generated the following spec for the function."
    chat_message = ChatMessage.from_actor(content, actor_id)

    return chat_message, code_spec


async def code_exec_completion(
        messages:list[ChatMessage],
        code_spec:SpecifyCode,
        actor_id:ActorId|None=None,
        ) -> tuple[ChatMessage, ExecuteCode | None]:
    
    system_message = normalize_prompt("""
        You are a helpful assistant. 
        Our goal is to execute a code function that we just generated. See the message history to see the type of task that we generated a function for.
        Now, we want to call that function. 
                                        
        Also, this system uses a simple key-value store that is accessed with simple string IDs. Which, if they contain, media, can be rendered in the UI.
        So, if the function is changing data, retrieving data, or generating content, you can expect it to return not the actual data but just an id to the result.
                                        
        Taking into consideration the conversation history, ask the user for information that can be used as input to the function. 
        Once you are certain that you have sufficient information, generate the function call
        """)
    messages_completion = []
    messages_completion.append({
        'role': 'system',
        'content': system_message
    })
    for msg in messages:
        messages_completion.append({
            'role': msg.from_name,
            'content': msg.content
        })
    response:OpenAIObject = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        messages=messages_completion,
        temperature=0.7,
        functions=[
            {
                "name": "entry",
                "description": normalize_prompt(f"""
                    This function accomplishes the following tasks: {code_spec.task_description}
                    """),
                "parameters": code_spec.arguments_spec, 
            }
        ],
    )

    print("code_exec_completion", response['choices'][0])
    exec_code = None
    if 'function_call' in response['choices'][0]['message']:
        function_call = response['choices'][0]['message']['function_call']
        function_args_str = function_call['arguments']
        function_args = json.loads(function_args_str)
        
        print('function_args', function_args)
        exec_code = ExecuteCode(input_arguments=function_args)
            
    content = response['choices'][0]['message'].get('content')
    if content is None and exec_code is not None:
        content = "I generated the following execution instructions."
    chat_message = ChatMessage.from_actor(content, actor_id)

    return chat_message, exec_code