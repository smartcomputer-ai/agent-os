import mistune
from uuid import UUID, uuid1
from datetime import datetime
from pydantic import BaseModel
from wit import *

#==============================================================
# Chat Messages
#==============================================================
class ChatMessage(BaseModel):
    id: UUID
    content: str
    timestamp: datetime
    from_name: str
    from_id: str|None = None

    @property
    def html(self):
        return mistune.html(self.content)

    @classmethod
    def from_user(cls, content:str):
        return cls(id=uuid1(), content=content, from_name='user', timestamp=datetime.now())
    
    @classmethod
    def from_actor(cls, content:str, actor_id:ActorId|None = None):
        return cls(
            id=uuid1(), content=content, from_name='assistant', from_id=actor_id.hex() if actor_id else None, timestamp=datetime.now())

    @classmethod
    async def load_from_tree(cls, tree:TreeObject, message_filter:list[str]=None) -> list['ChatMessage']:
        message_keys = tree.keys()
        message_keys = filter_and_sort_message_keys(message_keys, message_filter)
        messages = []
        for k in message_keys:
            blob_obj = await tree.getb(k)
            msg = cls(**blob_obj.get_as_json())
            messages.append(msg)
        return messages

def filter_and_sort_message_keys(message_keys:list[str], message_filter:list[str]|None = None) -> list[str]:
    #cleanup the filters
    if(message_filter is None):
            message_filter = []
    if(isinstance(message_filter, str)):
        message_filter = [message_filter]
    message_filter = [k for k in message_filter if k != 'null' and k != 'undefined']
    #filter the actual message keys
    if(len(message_filter) > 0):
        message_keys = [k for k in message_keys if k in message_filter]
    #convert keys to UUIDs (for sorting)
    message_keys = [UUID(k) for k in message_keys]
    message_keys = sorted(message_keys, key= lambda x: x.time)
    #convert back to string
    return [str(k) for k in message_keys]


