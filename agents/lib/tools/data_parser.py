import openai
from completions import *

class DataParser:

    async def parse(self, input:str, output_schema:dict|str, query:str) -> dict:
        # print("parse input", input)
        # print("parse output_schema", output_schema)
        # print("parse query", query)
        prompt = PromptBuilder()

        prompt.append_system(
            """You are a very smart assistant. Your task is to parse the given input string into the provided schema.""")
        prompt.append_msg(
            """The input string can be anything. For example, it can be parsed HTML from a website, JSON, prose text, or even code.
            Whatever the format is, make sure to take into account the desired output schema and the natural language query.""")
        
        prompt.append_msg(
            """Here is the input string:""")
        prompt.append_to_prev_code(input, codetype="")

        prompt.append_msg(
            "Here is natural language query: "+query)
        
        funcs = FunctionBuilder()
        funcs.register_function(
            "structured", 
            "Based on the context so far and the input string, the structured function takes this data as input.", 
            output_schema)
        
        response = await openai.ChatCompletion.acreate(
            model="gpt-4-0613",
            temperature=0,
            **build_chat_kwargs(prompt, funcs, function_call="structured"),
            )
        
        fn, arguments = parse_function_completion(response)
        if fn != "structured":
            raise Exception(f"Expected function name structured, but got {fn}")
        return arguments