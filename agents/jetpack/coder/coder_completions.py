import openai
from completions import *

async def inputoutput_completion(
        task_description:str, 
        input_examples: list[str]|None = None,
        input_spec:dict|None=None, 
        output_spec:dict|None=None,
        ) -> tuple[dict, dict]:

    if input_spec is not None and output_spec is not None:
        raise Exception("Both input and output spec already specified, nothing to do.", input_spec, output_spec)

    prompt = PromptBuilder()
    prompt.append_system(
        """You are an expert coder and you assist with generating Python code. 
        Our goal is to write some Python code and flesh out the specifications of what that code should do.

        Define the input and output structures of the desired function using JSON schemas. 
        The generated function should take all inputs as a single object, not as several parameters or agruments, and return a single object as output.
        The new code can understand all kinds of input in string or other basic formats, for example to download an image, it could just take a URL as input.
                                      
        Also, this system uses a simple key-value store that is accessed with simple string IDs. Which, if they contain, media, can be rendered in the UI.
        So, if the function is changing data, retrieving data, or generating content, ask it to save the data in the data store, and then specify that it just returns the id of the stored data and not the data itself, unless instructed otherwise.
        For example: instead of {"image_url": "https://example.com/image1.jpg"} return {"id": "<image_id>"}
        """)
    
    prompt.append_msg("Here is the task that the Python function needs to accomplish: ")
    prompt.append_to_prev(task_description)

    if input_examples is not None:
        prompt.append_msg("Here are some examples of possible inputs:")
        for input_example in input_examples:
            prompt.append_to_prev(input_example)

    func_params = {
        'properties': {}, 
        'required': [], 
        'title': 'CodeSpec', 
        'type': 'object'
        }
    
    if input_spec is not None:
        prompt.append_msg("The input schema has already been defined:")
        prompt.append_to_prev_json(input_spec)
    else:
        func_params['properties']['input_spec'] = {'type': 'object', 'title': 'Input Spec'}
        func_params['required'].append('input_spec')

    if output_spec is not None:
        prompt.append_msg("The output schema has already been defined:")
        prompt.append_to_prev_json(output_spec)
    else:
        func_params['properties']['output_spec'] = {'type': 'object', 'title': 'Output Spec'}
        func_params['required'].append('output_spec')
    
    funcs = FunctionBuilder()
    funcs.register_function("code_gen", None, func_params)
    funcs.append_to_last_description("""
        This function generates code according to the given specification (task_description).
        If the specification warrants inputs, provide an input_spec in JSON schema format.
        If the specification warrants outputs, provide a output_spec in JSON schema format.""")
    
    response = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        temperature=0.7,
        **build_chat_kwargs(prompt, funcs, function_call="code_gen"),
    )

    fn, args = parse_function_completion(response)
    if fn != "code_gen":
        raise Exception(f"Expected function name code_gen, but got {fn}")
    return args.get('input_spec'), args.get('output_spec')

async def code_completion(
        task_description:str, 
        plan:str|None=None, 
        data_examples: dict[str,str]|None = None,
        input_spec:dict|None=None, 
        output_spec:dict|None=None, 
        previous_code:str|None=None, 
        previous_code_errors:str|None=None,
        ) -> str:
    
    prompt = PromptBuilder()
    prompt.append_system(
        """You are an expert coder and you assist with generating Python code. 
        Please write code for the requested feature, and only the code in question with no premble or explanation. 
        The code is executed inside a sandboxed environment, which makes this very safe.
        Write an entire python module with imports, etc. You can write more than one function.
        But there must be an entry point function, called 'entry', that follows the requested 
        arguments and return type.""")
    
    prompt.append_msg("If you must explain something, please do so in a comment. And always wrap the Python code in a code block like so:\n```python\n#your code here\n```")

    prompt.append_msg(
        """All data is stored in a simple key-value store that is accessed with simple string IDs. 
        When persisting data, the store generates the ids. The inteface of the store is:""")
    prompt.append_to_prev_code(
        """
        class StoreWrapper:
            async def load_bytes(self, id:str) -> bytes | None:
                pass
            async def load_str(self, id:str) -> str | None:
                pass
            async def load_json(self, id:str) -> dict | None:
                pass
            async def store_bytes(self, data:bytes, content_type:str|None=None) -> str:
                pass
            async def store_str(self, data:str) -> str | None:
                pass
            async def store_json(self, data:dict) -> str | None:
                pass
        """)
    prompt.append_to_prev(
        """An instance of the store can be passed to your the function you write. For example: `async def entry(..., store:StoreWrapper)`. 
        The StoreWrapper type can be imported with `from tools import StoreWrapper`, but do not import it and do not type annotate it.
        Many times the input spec, if there is one, will contain ids to define the content that needs to be loaded. Use the store to load those ids.
        Many times the output spec, if there is one, will contain fields to return changed data. Use the store to store those fields and populate the ids in the return values.
        """)
    
    prompt.append_msg(
        """If there is data that needs parsing into a fixed and structured format, use the following parsing library:""")
    prompt.append_to_prev_code(
        """
        class DataParser:
            async def parse(self, input:str, output_schema:dict, query:str) -> dict:
                pass
        """)
    prompt.append_to_prev(
        """Create an instance of the DataParser whenever you need it. 
        The DataParser type can be imported with `from tools import DataParser`.
        It is up to you to define the output schema, which must be provided in JSON schema format.
        Make sure to define the output_schema generic enough that it can be used with various queries.
        Instead of using BeautifulSoup, use this library to parse HTML.
        If the user provides the query, then pass it to the `parse` function. 
        If only one kind of parsing is needed, then you can hardcode the query.
        For example:
        """)
    prompt.append_to_prev_code(
        """
        response = requests.get(url)
        parser = DataParser()
        output = await parser.parse(response.text, {"type": "object", "properties": {"name": {"type": "string"}}}, "find the name of the company")
        """)

    prompt.append_msg(
        """The functions in the module can have side effects. So, in the code we write, you can call you to external APIs, databases, file system and so on.
        Use whatever library needed to accomplish the task defined below.
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
    prompt.append_to_prev(", ".join(third_party_modules))

    if data_examples is not None:
        for data_location, data_example in data_examples.items():
            prompt.append_msg("Here is an example of data at location: "+data_location)
            prompt.append_to_prev_code(data_example, codetype="")

    prompt.append_msg("Here is the task that the module needs to accomplish: ")
    prompt.append_to_prev(task_description)

    if plan is not None:
        prompt.append_msg("Here is the plan on how to accomplish this:")
        prompt.append_to_prev(plan)

    prompt.append_msg("Let's describe the desired input and output of the entry function.")
    if input_spec:
        prompt.append_to_prev("The entry function needs to take an argument called `input` and it is of the following structure, treat it as a dict:")
        prompt.append_to_prev_json(input_spec)
    else:
        prompt.append_to_prev("The entry function does not take any arguments besides the store.")

    if output_spec:
        prompt.append_to_prev("The entry function needs return an object of the following structure, return it as a dict:")
        prompt.append_to_prev_json(output_spec)
    else:
        prompt.append_to_prev("The entry function should not return a value, try to accomplish the task entirely with side-effects.")

    if previous_code is not None:
        prompt.append_msg("Here is the code from previous attempts:")
        prompt.append_to_prev_code(previous_code)
        
    if previous_code_errors is not None:
        prompt.append_msg("Here are the errors from that code:")
        prompt.append_to_prev(previous_code_errors)

    response = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        temperature=0.3,
        **build_chat_kwargs(prompt),
        )
    return parse_code_completion(response)


async def function_call_completion(
        function_name:str, 
        function_description:str, 
        parameters:dict|None, 
        input_description:str|None=None,
        ) -> dict:
    
    prompt = PromptBuilder()
    prompt.append_system(
        """You are an expert coder and you assist with generating Python code. 
        You generated the following function, and now we create a function call for it.""")
    prompt.append_msg(
        "The data and type of input to generate for the function call is as follows:")
    prompt.append_to_prev(input_description)

    funcs = FunctionBuilder()
    funcs.register_function(function_name, function_description, parameters)

    response = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        temperature=0.3,
        **build_chat_kwargs(prompt, funcs, function_call=function_name),
        )
    
    fn, arguments = parse_function_completion(response)
    if fn != function_name:
        raise Exception(f"Expected function name {function_name}, but got {fn}")
    return arguments



#------------------------------------------------------------
#WIP

async def plan_completion(
        task_description:str, 
        arguments_spec:dict|None=None, 
        return_spec:dict|None=None, 
        test_descriptions: list[str]|None = None,
        ) -> str:
    
    prompt = PromptBuilder()
    prompt.append_system(
        """You are an expert coder and you assist with generating Python code. 
        But let's not write the code just yet. Let's plan it out first. Think step-by-step.
        However, sometimes the task description or the input/output spec is referencing something that contains structured data, which needs to be understood first before the code can be properly planned. 
        In that case retrieve that data first, and only then plan the code to be written."""
        )
    
    prompt.append_msg(
        """An example where the data should be requested and inspected:
        'Download this CSV and tell sum the totals column for each country. http://samplecsvs.s3.amazonaws.com/SalesJan2009.csv'""")
    
    prompt.append_msg(
        """An example where the data should not be requested, because the image binary is opaque:
        'Resize this image to 500x500 pixels. https://images.example.com/12345.jpg'""")
    
    #copied from coder_completions.py
    prompt.append_msg(
        """All data is stored in a simple key-value store that is accessed with simple string IDs. 
        When persisting data, the store generates the ids. The inteface of the store is:""")
    prompt.append_to_prev_code(
        """
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
        """)
    prompt.append_to_prev(
        """An instance of the store can be passed to your the function you write. For example: `async def entry(..., store:StoreWrapper)`. 
        Many times the input spec--if there is one--will contain ids to define the content that needs to be loaded. Use the store to load those ids.
        Many times the output spec--if there is one--will contain fields to return changed data. Use the store to store those fields and populate the ids in the return values.
        If you know the content type (or mime type) of binary data, then you can pass it to the store as well.""")
    
    prompt.append_msg(
        """If there is data that needs parsing into a fixed and structured format, use the following parsing library:""")
    prompt.append_to_prev_code(
        """
        class DataParser:
            async def parse(self, input:str, output_schema:dict, query:str) -> dict:
                pass
        """)
    prompt.append_to_prev(
        """Create an instance of the DataParser whenever you need it. 
        The DataParser type can be imported with `from tools import DataParser`.
        It is up to you to define the output schema, which must be provided in JSON schema format.
        Make sure to define the output_schema generic enough that it can be used with various queries.
        Instead of using BeautifulSoup, use this library to parse HTML.
        If the user provides the query, then pass it to the `parse` function. 
        If only one kind of parsing is needed, then you can hardcode the query.
        For example:
        """)
    prompt.append_to_prev_code(
        """
        response = requests.get(url)
        parser = DataParser()
        output = await parser.parse(response.text, {"type": "object", "properties": {"name": {"type": "string"}}}, "find the name of the company")
        """)

    prompt.append_msg(
        """The functions in the module can have side effects. So, in the code we write, you can call you to external APIs, databases, file system and so on.
        Use whatever library needed to accomplish the task defined below.
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
    prompt.append_to_prev(", ".join(third_party_modules))
    #end copied

    prompt.append_msg("Here is the task that the Python module needs to accomplish: " + task_description)

    prompt.append_msg("Let's describe the desired input and output of the entry function.")
    if arguments_spec:
        prompt.append_to_prev("The entry function needs to take an argument called `input` and it is of the following structure, treat it as a dict:")
        prompt.append_to_prev_json(arguments_spec)
    else:
        prompt.append_to_prev("The entry function does not take any arguments besides the store.")

    if return_spec:
        prompt.append_to_prev("The entry function needs return an object of the following structure, return it as a dict:")
        prompt.append_to_prev_json(return_spec)
    else:
        prompt.append_to_prev("The entry function should not return a value, try to accomplish the task entirely with side-effects.")

    if test_descriptions:
        prompt.append_msg("The following describes some of the expected inputs:")
        for test_description in test_descriptions:
            prompt.append_msg(test_description)

    prompt.append_msg("Based on the information here, decide to either make a plan, thinking step by step, or request the structure of the data via a function call.")

    response = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        temperature=0.2,
        **build_chat_kwargs(prompt),
    )

    completion = parse_completion(response)
    return completion