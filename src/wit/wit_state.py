import inspect
import pickle
from abc import ABC
from typing import TypeVar
from grit import *
from .data_model import *
from .data_model_utils import *

# Load and persist state data to a core
#TODO: make this much smarter to track what actually changed, and what did not

class WitState(ABC):
    """Loads and persists any class and instance properties that a subclass adds to itself.
    
    Use this utility class to manage more complicated wit state.
    """
    def __init__(self, core_path:str='/state') -> None:
        super().__init__()
        self._core_path = core_path

    def _include_attribute(self, attr_key:str):
        """Returns true if the attribute should be included in the state"""
        attr = getattr(self, attr_key)
        return (
            not attr_key.startswith('_') 
            and not attr_key.startswith('__') 
        )
    
    def _before_load(self):
        """Called before loading state from core"""
        pass

    def _after_load(self):
        """Called after loading state from core"""
        pass

    def _before_persist(self):
        """Called before persisting state to core"""
        pass

    def _after_persist(self):
        """Called after persisting state to core"""
        pass

    async def _load_from_core(self, core:Core):
        self._before_load()
        attributes = dir(self)
        state = core.maket_path(self._core_path, exist_ok=True)
        for attr_key in attributes:
            if self._include_attribute(attr_key):
                #print(f'loading {attr_key}')
                #try to find the blob
                property_data = await state.get(attr_key)
                if property_data is not None:
                    if(not isinstance(property_data, BlobObject)): #we expect this to be a blob object
                        raise Exception(f'Expected property {attr_key} to be a BlobObject, but got {type(property_data)}')     
                    self.__setattr__(attr_key, pickle.loads(property_data.get_as_bytes()))
        self._after_load()

    async def _persist_to_core(self, core:Core):
        self._before_persist()
        attributes = dir(self)
        state = core.maket_path(self._core_path, exist_ok=True)
        for attr_key in attributes:
            if self._include_attribute(attr_key):
                property_data = await state.getb(attr_key)
                attr_value = self.__getattribute__(attr_key)
                if(attr_value is not None):
                    #print(f'persisting {attr_key}')
                    property_data.set_as_bytes(pickle.dumps(attr_value))
                else:
                    property_data.set_empty()
        self._after_persist()
