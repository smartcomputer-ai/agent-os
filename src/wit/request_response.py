
from abc import ABC, abstractmethod
from grit import *
from wit import *

class RequestResponse(ABC):
    @abstractmethod
    async def run(
        self, 
        msg:OutboxMessage, 
        sender_id:ActorId, 
        response_types:list[str], 
        timeout:float|None = None,
        ) -> InboxMessage:
        pass
