
from abc import ABC, abstractmethod
from aos.grit import *

class Presence(ABC):
    """A service to check the presence of a user (or any entity) on an arbitrary channel.
    This is used to communicate out of band during execution of a wit function.

    For example, when streaming an LLM completion inside a wit, this can be used to return the text stream
    to the user while executing the function as a single step."""
    
    @abstractmethod
    async def check(self, channel:str) -> bool:
        """Checks if anyone is present on this channel."""
        pass

    async def publish(self, channel:str, message:Blob) -> None:
        """Publishes a message to the channel."""
        pass