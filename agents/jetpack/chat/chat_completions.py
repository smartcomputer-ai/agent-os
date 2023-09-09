import openai
from grit import *
from completions import *
from jetpack.messages import ChatMessage, CodeRequest, CodeSpec, CodeExecution

async def chat_completion(
        messages:list[ChatMessage],
        code_spec:CodeSpec|None=None,
        ) -> str | CodeRequest | CodeExecution:
    
    if(openai.api_key is None):
        return "I am a helpful assistant, and I want to reply to your question, but the OpenAI API key is not set" 

    prompt = PromptBuilder()
    prompt.append_system(
        """You are a helpful assistant. 
        Our goal is to write some Python code and flesh out the specifications of what that code should do.
        Use to code gen function when you believe you have enough information to summarize the desired functionality and pass it as code generation request (code_request).
        As long as you are not certain, ask clarifying questions.

        Also, this system uses a simple key-value store that is accessed with simple string IDs. Which, if they contain, media, can be rendered in the UI.
        So, if the function is changing data, retrieving data, or generating content, you can expect it to return not the actual data but just an id to the result.

        Since we are generating code, it is also necessary to have an example of how to call the function. 
        For example, if the function is called "download_image", then you should ask the user to give an example of a real URL to use for testing.

        You can access the web, internal URLs, the file system, and so on, because you can write and execute code.

        Once the code is written, either request changes (code_request) to the code because the user mentioned changes, or call the code because the user asked for a code execution (code_exec).

        Whenever the user asks you modify or call a function make the actual function call--do not just copy previous output.
        """)
    
    for msg in messages:
        prompt.append_msg(msg.content, msg.from_name, clean_whitespace=False)

    funcs = FunctionBuilder()
    funcs.register_function("code_request", None, CodeRequest)
    funcs.append_to_last_description(
        """This function generates code according to the given specification (task_description).
        If the specification warrants tests, provide a list of input_examples. Often one test is enough.""")
    
    if code_spec is not None:
        if code_spec.input_spec is None:
            raise Exception("chat completions: code_spec.input_spec is None")
        funcs.register_function("code_exec", None, code_spec.input_spec)
        funcs.append_to_last_description("""This function accomplishes the following task:""")
        funcs.append_to_last_description(code_spec.task_description)

    response = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        temperature=0.5,
        **build_chat_kwargs(prompt, funcs),
    )

    completion = parse_completion(response)
    if isinstance(completion, str):
        return completion
    else:
        #sometimes, tests descriptions are not strings, convert them to strings
        fn, args = completion
        if fn == "code_request":
            if args['input_examples'] is not None:
                args['input_examples'] = [str(example) for example in args['input_examples']]
            code_request = CodeRequest(**args)
            return code_request
        elif fn == "code_exec":
            code_exec = CodeExecution(input_arguments=args)
            return code_exec