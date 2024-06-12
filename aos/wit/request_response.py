
from abc import ABC, abstractmethod
from .data_model import OutboxMessage, InboxMessage

class RequestResponse(ABC):
    @abstractmethod
    async def run(
        self, 
        msg:OutboxMessage, 
        response_types:list[str], 
        timeout:float|None = None,
        ) -> InboxMessage:
        pass
