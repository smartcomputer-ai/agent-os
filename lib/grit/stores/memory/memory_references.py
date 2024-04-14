from grit.object_model import *
from grit.references import References

class MemoryReferences(References):
    #no lockign needed here, because all the dict operations used here are atomic
    _ref:dict[str, ObjectId]

    def __init__(self):
        super().__init__()
        self._ref = {}

    async def get(self, ref:str) -> ObjectId | None:
        return self._ref.get(ref, None)
    
    async def get_all(self) -> dict[str, ObjectId]:
        return self._ref.copy()

    async def set(self, ref:str, object_id:ObjectId) -> None:
        self._ref[ref] = object_id

    def get_sync(self, ref:str) -> ObjectId | None:
        return self._ref.get(ref, None)

    def get_all_sync(self) -> dict[str, ObjectId]:
        return self._ref.copy()

    def set_sync(self, ref:str, object_id:ObjectId) -> None:
        self._ref[ref] = object_id