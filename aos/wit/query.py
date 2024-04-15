
from abc import ABC, abstractmethod
from aos.grit import *

class Query(ABC):
    @abstractmethod
    async def run(
        self, 
        actor_id:ActorId, 
        query_name:str, 
        context:Blob|None,
        ) -> Tree | Blob:
        pass