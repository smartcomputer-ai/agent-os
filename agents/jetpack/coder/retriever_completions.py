import logging
import openai
from pydantic import BaseModel
from completions import *

logger = logging.getLogger(__name__)

async def retrieve_completion(
        task_description:str, 
        input_examples: list[str]|None = None,
        ) -> list[str] | None:
    
    prompt = PromptBuilder()
    prompt.append_system(
        """You are an expert coder and you assist with generating Python code. 
        But let's not write the code just yet. Let's plan it out first.
        However, sometimes the task description or the input/output spec is referencing something that contains structured data, 
        which needs to be understood first before the code can be properly planned. 
        In that case request that data first, and only then plan the code to be written.
        Often more than one item needs to be investigated, both from the requested input and output, ensure you capture all of them."""
        )
    
    prompt.append_msg("""Here are a few examples of the locations that should be extracted from task descriptions:""")

    prompt.append_to_prev("'Download this CSV and tell sum the totals column for each country. http://samplecsvs.s3.amazonaws.com/SalesJan2009.csv'")
    prompt.append_to_prev("returns: ['http://samplecsvs.s3.amazonaws.com/SalesJan2009.csv']")
    
    prompt.append_to_prev("'Resize this image, https://images.example.com/12345.jpg, to 500x500 pixels.'")
    prompt.append_to_prev("returns: []")

    prompt.append_to_prev("'Resize this image to 500x500 pixels. https://images.example.com/12345.png. And then append the file name to the following CSV /home/downloaded/images.csv'")
    prompt.append_to_prev("returns: ['/home/downloaded/images.csv']")

    prompt.append_to_prev("Combine the following three text files into one file: /home/my/text1.txt, /home/my/text2.txt, /lib/text3.txt")
    prompt.append_to_prev("returns: ['/home/my/text1.txt', '/home/my/text2.txt', '/lib/text3.txt']")

    prompt.append_msg("Here is the task that the Python module needs to accomplish: " + task_description)

    if input_examples:
        prompt.append_msg("The following describes some of the expected inputs:")
        for test_description in input_examples:
            prompt.append_msg(test_description)

    prompt.append_msg(
        """Based on the information here, decide to either make a plan, or request the structure of the data via a function call.
        If you need to plan, then just say 'plan' and we'll work on a plan later. 
        Otherwise call the retrieve function with the data pieces you need.""")

    class DataRetrieval(BaseModel):
        locations: list[str]
        
    funcs = FunctionBuilder()
    funcs.register_function(
        "retrieve", 
        "Retrieve data from the following locations. These are the all the pieces of data that need to be investigated based on the context so far.", 
        DataRetrieval)

    response = await openai.ChatCompletion.acreate(
        model="gpt-4-0613",
        temperature=0.2,
        **build_chat_kwargs(prompt, funcs),
        )

    completion = parse_completion(response)

    if isinstance(completion, str):
        logger.debug("No data retrieval needed. The models answered with:", completion)
        return None
    else:
        fn, args = completion
        if fn != "retrieve":
            raise Exception(f"Expected function name retrieve, but got {fn}")
        retrieval = DataRetrieval(**args)
        return retrieval.locations