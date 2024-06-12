from __future__ import annotations
from aos.grit import *
from aos.wit.discovery import Discovery

class DiscoveryExecutor(Discovery):
    """Executes searches to find other actors."""
    references:References
    def __init__(self, references:References):
        self.references = references

    async def find_named_actor(self, actor_name:str) -> ActorId | None:
        return await self.references.get(ref_actor_name(actor_name))

    async def find_prototype(self, prototype_name:str) -> ActorId | None:
        return await self.references.get(ref_prototype_name(prototype_name))


