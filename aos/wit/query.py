
from abc import ABC, abstractmethod
from typing import Type
from aos.grit import *
from aos.wit import BlobObject, BaseModelType
from pydantic import BaseModel

class Query(ABC):
    @abstractmethod
    async def _run(
            self, 
            actor_id:ActorId, 
            query_name:str, 
            context:Blob|None,
            ) -> Tree | Blob | None:
        pass

    async def run(
            self, 
            actor_id:ActorId, 
            query_name:str, 
            query_context:Blob|BlobObject|BaseModel|dict|str|None = None,
            ) -> Tree | Blob | None:
        if query_context is not None:
            if isinstance(query_context, BlobObject):
                query_context = query_context.get_as_blob()
            elif isinstance(query_context, BaseModel) or isinstance(query_context, dict):
                query_context = BlobObject.from_json(query_context).get_as_blob()
            elif isinstance(query_context, str):
                query_context = BlobObject.from_str(query_context).get_as_blob()
            elif is_blob(query_context):
                query_context = query_context
            else:
                raise ValueError("query_context must be a Blob, BlobObject, BaseModel, dict, or str.")
        return await self._run(actor_id, query_name, query_context)
    
    async def run_as_model(
            self, 
            actor_id:ActorId, 
            query_name:str,
            pydantic_model:Type[BaseModelType], 
            query_context:Blob|BlobObject|BaseModel|dict|str|None = None,
            ) -> BaseModelType | None:
        result = await self.run(actor_id, query_name, query_context)
        if result is None:
            return None
        if not is_blob(result):
            raise ValueError(f"Query result must be a blob, cannot convert to {pydantic_model}.")
        result = BlobObject.from_blob(result)
        return result.get_as_model(pydantic_model)