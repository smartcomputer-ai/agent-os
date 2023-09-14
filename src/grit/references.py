from abc import ABC, abstractmethod
from grit.object_model import ObjectId, ActorId

class References(ABC):
    """Interface for saving and quering references in the Grit object store."""
    @abstractmethod
    async def get(self, ref:str) -> ObjectId | None:
        pass

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
    return f"actors/{actor_name_ref}"

def ref_runtime_actor_name(actor_name_ref:str) -> str:
    return f"runtime/{actor_name_ref}"

def ref_runtime_agent() -> str:
    return "runtime/agent"
