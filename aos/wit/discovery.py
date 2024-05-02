
from abc import ABC, abstractmethod
from aos.grit import *

class Discovery(ABC):
    @abstractmethod
    async def find_named_actor(self, actor_name:str) -> ActorId | None:
        pass

    @abstractmethod
    async def find_prototype(self, prototype_name:str) -> ActorId | None:
        pass