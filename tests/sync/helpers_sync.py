import os

from grit.stores.memory.memory_object_store import MemoryObjectStore
from src.grit import *
from src.wit import *
from src.sync import *

def get_random_actor_id() -> ActorId:
    return get_object_id(os.urandom(20))

def create_file(path, file_name, content: str|bytes|dict):
    print("path type", path)
    if os.name == "nt" and "/" in str(path):
        path = path.replace("/", os.sep)
    os.makedirs(path, exist_ok=True)
    if(isinstance(content, dict)):
        content = json.dumps(content)
    if(isinstance(content, str)):
        content = content.encode('utf-8')
    with open(os.path.join(path, file_name), "wb") as f:
        f.write(content)
    return os.path.join(path, file_name)