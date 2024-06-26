from abc import ABC, abstractmethod
from .object_model import ObjectId, ActorId

class References(ABC):
    """Interface for saving and quering references in the Grit object store."""
    @abstractmethod
    async def get(self, ref:str) -> ObjectId | None:
        pass

    #todo: refactor this to "get_refs", with ref prefix
    @abstractmethod
    async def get_all(self) -> dict[str, ObjectId]:
        pass

    @abstractmethod
    async def set(self, ref:str, object_id:ObjectId) -> None:
        pass

    @abstractmethod
    def get_sync(self, ref:str) -> ObjectId | None:
        pass

    @abstractmethod
    def get_all_sync(self) -> dict[str, ObjectId]:
        pass

    @abstractmethod
    def set_sync(self, ref:str, object_id:ObjectId) -> None:
        pass

# Helper functions to create correcly formated references
def ref_step_head(actor_id:ActorId|str) -> str:
    if(isinstance(actor_id, ActorId)):
        actor_id = actor_id.hex()
    return f"heads/{actor_id}"

def ref_actor_name(actor_name_ref:str) -> str:
    if actor_name_ref == "root":
        raise ValueError("Actor name 'root' is reserver for the runtime root actor.")
    return f"actors/{actor_name_ref}"

def ref_prototype_name(prototype_name_ref:str) -> str:
    return f"prototypes/{prototype_name_ref}"

#todo: rename to "root_actor"
def ref_root_actor() -> str:
    return "runtime/agent"
