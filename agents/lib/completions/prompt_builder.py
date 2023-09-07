
import json
import inspect
from pydantic import BaseModel

class PromptBuilder:
    def __init__(self):
        self.system_message = ""
        self.messages = []

    def build(self) -> list[dict]:
        messages = []
        if self.system_message:
            messages.append({
                'role': 'system',
                'content': self.system_message
            })
        messages += self.messages
        return messages


    def _clean_message(self, message:str, clean_whitespace:bool=True) -> str:
        if message is None:
            return ""
        if clean_whitespace:
            return inspect.cleandoc(message)
            #return "\n".join(line.strip() for line in message.splitlines())
        else:
            return message

    def append_system(self, message:str):
        self.system_message += self._clean_message(message) + "\n"
        return self

    def append_msg(self, message:str, role:str='user', clean_whitespace:bool=True):
        self.messages.append({
            'role': role,
            'content': self._clean_message(message, clean_whitespace)
        })
        return self

    def append_to_prev(self, message:str, newline:bool=True, clean_whitespace:bool=True):
        """Append to the previous message. If there is no previous message, raise an error."""
        if len(self.messages) == 0:
            raise ValueError("Must have at least one message to append new message to")
        if newline:
            self.messages[-1]['content'] += "\n"
        self.messages[-1]['content'] += self._clean_message(message, clean_whitespace)
        return self
    
    def append_to_prev_code(self, code:str, newline:bool=True, codetype:str="python"):
        if len(self.messages) == 0:
            raise ValueError("Must have at least one message to append code to")
        if "```" not in code:
            code = f"```{codetype}\n"+self._clean_message(code)+"\n```"
        if newline:
            self.messages[-1]['content'] += "\n"
        self.messages[-1]['content'] += code
        return self
    
    def append_to_prev_json(self, data:dict|BaseModel, newline:bool=True):
        if isinstance(data, BaseModel):
            data_str = data.model_dump_json()
        elif isinstance(data, dict):
            data_str = json.dumps(data)
        else:
            raise ValueError("Must be dict or BaseModel")
        return self.append_to_prev_code(json.dumps(data_str), newline=newline, codetype="json")

