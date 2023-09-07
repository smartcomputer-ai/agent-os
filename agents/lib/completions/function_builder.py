import inspect
import json
from typing import Type
from pydantic import BaseModel

class FunctionBuilder:
    def __init__(self):
        self.functions = []

    def build(self) -> list[dict]:
        return self.functions

    def _clean_message(self, message:str, clean_whitespace:bool=True) -> str:
        if message is None:
            return ""
        if clean_whitespace:
            return inspect.cleandoc(message)
            #return "\n".join(line.strip() for line in message.splitlines())
        else:
            return message

    def register_function(self,
        name:str,
        description:str,
        parameters:dict|Type[BaseModel],
        ):
        if parameters is None:
            parameters = json.loads('{"type": "object", "properties": {}}')
        if inspect.isclass(parameters) and issubclass(parameters, BaseModel):
            parameters = parameters.model_json_schema()
        self.functions.append({
            "name": name,
            "description": self._clean_message(description),
            "parameters": parameters
        })
        return self

    def append_to_last_description(self, description:str, newline:bool=True, clean_whitespace:bool=True):
        if len(self.functions) == 0:
            raise ValueError("Must have at least one function to append description to")
        if newline:
            self.functions[-1]['description'] += "\n"
        self.functions[-1]['description'] += self._clean_message(description, clean_whitespace)
        return self
