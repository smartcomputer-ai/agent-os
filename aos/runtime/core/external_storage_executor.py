import os
from aos.wit.external_storage import ExternalStorage

class ExternalStorageExecutor(ExternalStorage):
    """Provides access to external storage."""
    def __init__(self, root_dir:str, agent_dir:str, actor_dir:str|None):
        self.root_dir = root_dir
        self.agent_dir = agent_dir
        self.actor_dir = actor_dir

    def get_dir(self, sub_dir:str|None=None) -> str:
        """Returns a directory where the actor can store files. The directory will be created if it does not exist."""
        if self.actor_dir is None:
            raise Exception("Actor directory not set. Make a storage executor for this actor.")
        if sub_dir:
            dir = os.path.join(self.root_dir, self.agent_dir, self.actor_dir, sub_dir)
        else:
            dir = os.path.join(self.root_dir, self.agent_dir, self.actor_dir)
        if not os.path.exists(dir):
            os.makedirs(dir, exist_ok=True)
        return dir
    
    def make_for_actor(self, actor_dir:str) -> ExternalStorage:
        """Creates an external storage executor for the actor."""
        return ExternalStorageExecutor(self.root_dir, self.agent_dir, actor_dir)