
from abc import ABC, abstractmethod

class ExternalStorage(ABC):
    """Provides access to external storage. External here means not Grit. So a file system or a cloud storage."""
    @abstractmethod
    def get_dir(self, sub_dir:str|None=None) -> str:
        """Returns a directory where the actor can store files. The directory will be created if it does not exist."""
        pass
