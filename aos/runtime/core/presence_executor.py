from __future__ import annotations
from aos.grit import *
from aos.wit.presence import Presence

class NoOpPresenceExecutor(Presence):
    """Implements the presence interface as a no op.
    For a full implementation, see portals, which uses Redis.
    """

    async def check(self, channel:str) -> bool:
        """Checks if anyone is present on this channel."""
        return False

    async def publish(self, channel:str, message:Blob) -> None:
        """Publishes a message to the channel."""
        pass