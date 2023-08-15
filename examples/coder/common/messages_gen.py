
from uuid import UUID, uuid1
from pydantic import BaseModel
from datetime import datetime
from wit import *

#==============================================================
# Code Generation Messages
#==============================================================
class SpecifyCode(BaseModel):
    task_description: str
    arguments_spec: dict|None = None
    return_spec: dict|None = None
    max_code_tries:int|None = None
    test_descriptions: list[str]|None = None

class ExecuteCode(BaseModel):
    #provide one or the other, if both are provider, the arguments will be used
    input_arguments: dict|None = None
    input_description: str|None = None

class CodePlanned(BaseModel):
    task_description: str
    code_plan: str

class CodeDeployed(BaseModel):
    code: str

class CodeExecuted(BaseModel):
    input_arguments: dict
    output: dict