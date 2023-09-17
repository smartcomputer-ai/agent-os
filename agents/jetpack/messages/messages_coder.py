from pydantic import BaseModel

#==============================================================
# Code Generation Messages
#==============================================================
class CodeRequest(BaseModel):
    task_description: str
    input_examples: list[str]|None = None

class CodeSpec(BaseModel):
    task_description: str
    input_examples: list[str]|None = None
    data_examples: dict[str,str]|None = None
    input_spec: dict|None = None
    output_spec: dict|None = None

    @staticmethod
    def empty_inputoutput_spec() -> dict:
        return {"properties": {}, "type": "object" }

class CodePlanned(BaseModel):
    plan: str

class CodeDeployed(BaseModel):
    code: str

class CodeExecution(BaseModel):
    #provide one or the other, if both are provider, the arguments will be used
    input_arguments: dict|None = None
    input_description: str|None = None

class CodeExecuted(BaseModel):
    input_arguments: dict
    output: dict

class CodeFailed(BaseModel):
    errors: str
