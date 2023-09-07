from .prompt_builder import PromptBuilder
from .function_builder import FunctionBuilder
from .completion_helpers import *

def config():
    import os
    import openai
    from dotenv import load_dotenv
    load_dotenv()
    openai.api_key = os.getenv("OPENAI_API_KEY")

config()