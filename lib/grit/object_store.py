from abc import ABC, abstractmethod
from grit.object_model import *

class ObjectLoader(ABC):
    """Interface for loading objects from the Grit object store."""
    @abstractmethod
    async def load(self, objectId:ObjectId) -> Object | None:
        pass

    @abstractmethod
    def load_sync(self, objectId:ObjectId) -> Object | None:
        pass

class ObjectStore(ObjectLoader, ABC):
    """Interface for persisting objects in the Grit object store."""
    @abstractmethod
    async def store(self, object:Object) -> ObjectId:
        pass

    @abstractmethod
    def store_sync(self, object:Object) -> ObjectId:
        pass

