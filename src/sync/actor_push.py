from __future__ import annotations
import logging
import os
import filetype
import mimetypes  
from collections import OrderedDict
from typing import Iterable
from grit.object_serialization import blob_to_bytes
from grit import *
from wit import *
from runtime.runtime_executor import add_offline_message_to_runtime_outbox, remove_offline_message_from_runtime_outbox
from . sync_item import SyncItem, sync_from_push_path, sync_from_push_value

logger = logging.Logger(__name__)

# Utility class to push external data to a specific actor

class ActorPush():
    """Encapsulates a single push to a specific actor. 
    
    If the actor doesnt exist yet, it will create a genesis message.
    It the actor already exists it will generate an update messsage."""
    _actor_id:ActorId
    _is_genesis:bool
    _sync_items:OrderedDict[str, SyncItem]
    actor_name:str
    wit:str
    wit_query:str
    wit_update:str
    runtime:str
    notify:set[str]

    def __init__(self, is_genesis:bool, actor_id:ActorId|None=None):
        self._is_genesis = is_genesis
        self._actor_id = actor_id
        if(not is_genesis and actor_id is None):
            raise ValueError("Cannot create a non-genesis actor push without an actor id.")
        self._sync_items = OrderedDict()
        self.actor_name = None
        self.wit = None
        self.wit_query = None
        self.wit_update = None
        self.runtime = None
        self.notify = set()

    @classmethod
    async def from_actor_name(cls, references:References, actor_name:str) -> ActorPush:
        # check if the actor already exists
        actor_id = await references.get(ref_actor_name(actor_name))
        if(actor_id is not None):
            # the actor ref is a just helper reference, to make sure the actor 
            # actually exists (i.e. the genesis step has run) also check if there 
            # is a step head for this actor
            step_id = await references.get(ref_step_head(actor_id))
        else:
            step_id = None
        #now, either create a genesis push, or an update push
        if(step_id is None):
            obj = cls(is_genesis=True, actor_id=None)
        else:
            obj = cls(is_genesis=False, actor_id=actor_id)
        obj.actor_name = actor_name
        return obj

    @property
    def is_genesis(self) -> bool:
        return self._is_genesis
    
    @property
    def actor_id(self) -> ActorId:
        return self._actor_id

    @property
    def sync_items(self) -> list[SyncItem]:
        return list(self._sync_items.values())

    def _add_sync_item(self, item:SyncItem, allow_file_override:bool):
        # Since later paths can override earlier paths, keep them in a dict
        # However, new files, can only override other files, not directories.
        # So, you we are adding items, keep track of seen core paths and core item paths,
        # and if an item wants to be added as a core path or vice versa, do not allow override
        if not hasattr(self, '__seen_core_paths'):
            self.__seen_core_paths = set()
            self.__seen_core_item_paths = set()
        if item.core_path in self.__seen_core_item_paths:
            existing_item = self._sync_items[item.core_path]
            raise ValueError(f"Cannot add path '{item}' because it was already added as a core item earlier: '{existing_item}'.")
        if item.core_item_path in self.__seen_core_paths:
            existing_item = next((i for i in self._sync_items.values() if i.core_path == item.core_item_path), None)
            raise ValueError(f"Cannot add item '{item}' because it was already added as a core path earlier: '{existing_item}'.")
        if(item.core_item_path in self._sync_items and not allow_file_override):
            existing_item = self._sync_items[item.core_item_path]
            raise ValueError(f"Cannot add item '{item}' because it was already added as a file earlier: '{existing_item}'.")
        self._sync_items[item.core_item_path] = item
        self.__seen_core_paths.add(item.core_path)
        self.__seen_core_item_paths.add(item.core_item_path)

    def add_push_path(self, push_path:str, ignore:list[str]=None, allow_file_override=True) -> ActorPush:
        for item in sync_from_push_path(push_path, ignore):
            self._add_sync_item(item, allow_file_override)
        return self
    
    def add_push_value(self, core_path:str, value:any, allow_file_override=True) -> ActorPush:
        item = sync_from_push_value(core_path, value)
        self._add_sync_item(item, allow_file_override)
        return self

    def to_core(self) -> Core:
        core = Core(None, None, None)
        # add wit, wit_query, runtime
        if(self.wit):
            core.makeb("wit").set_as_str(self.wit)
        if(self.wit_query):
            core.makeb("wit_query").set_as_str(self.wit_query)
        if(self.wit_update):
            core.makeb("wit_update").set_as_str(self.wit_update)
        if(self.runtime):
            core.makeb("runtime").set_as_str(self.runtime)
        # add sync items, grouping by the core_path they will be added to
        groups = {} 
        for v in self._sync_items.values(): 
            groups.setdefault(v.core_path, []).append(v)
        for k,v in groups.items():
            core_path = k
            sync_items = v
            # the paths should already be checked to not overwrite each other
            node:TreeObject = core.maket_path(core_path, exist_ok=True) 
            sync_item:SyncItem
            for sync_item in sync_items:
                if(sync_item.has_item_value):
                    blob_obj = _blob_from_value(sync_item.item_value)
                else:
                    blob_obj = _blob_from_file(os.path.join(sync_item.dir_path, sync_item.file_name))
                node.add(sync_item.item_name, blob_obj)
        return core
    
    async def diff_core_with_actor(self, store:ObjectStore, references:References) -> AsyncIterator[tuple[str, str]]:
        if(self._is_genesis):
            push_core = self.to_core()
            async for path, _trees, blobs in push_core.walk():
                for blob in blobs:
                    push_blob_path = os.path.join(path, blob.breadcrumb)
                    yield push_blob_path, "genesis"
        else:
            # get the core of the last step of this actor
            step_id = await references.get(ref_step_head(self._actor_id))
            if(step_id is None):
                raise Exception(f"Cannot diff core with actor because actor has no steps: {self._actor_id}")
            step = await store.load(step_id)
            step_core:Core = await Core.from_core_id(store, step.core)
            push_core = self.to_core()
            # walk the push_core and see if the step_core is different
            async for path, _trees, blobs in push_core.walk():
                for blob in blobs:
                    push_blob_path = os.path.join(path, blob.breadcrumb)
                    try:
                        step_blob:BlobObject = await step_core.get_path(push_blob_path)
                        if(step_blob is None):
                            yield push_blob_path, "missing in step"
                        else:
                            #compare the values
                            push_blob_object_id = get_object_id(blob_to_bytes(blob.get_as_blob()))
                            step_blob_object_id = step_blob.get_as_object_id() #this only works if it's been persisted or loaded from the store
                            if(push_blob_object_id != step_blob_object_id):
                                yield push_blob_path, "object id mismatch"
                    except Exception as ex:
                        yield push_blob_path, f"error retrieving path: {ex}"

    async def create_and_inject_messages(self, store:ObjectStore, references:References, agent_name:str) -> StepId:
        # While pushing the first genesis core to actors, the initial wit can fail (not be found, code error, etc). 
        # If this is the case the developer will iterate on the push files and values and as a consequnce change the 
        # actor id of the target actor each time.
        # So, if this is a genesis push, look if there is already an agent ref for this actor, and if so, remove it.
        if(self._is_genesis):
            previous_genesis_actor_id = await references.get(ref_actor_name(self.actor_name))
            if(previous_genesis_actor_id is not None):
                await remove_offline_message_from_runtime_outbox(store, references, agent_name, previous_genesis_actor_id)
        # Now, create the message
        msg = await self.create_actor_message(store)
        # Don't set previous, so that this message can be pushed multiple times, 
        # resulting in an override of the current message in the outbox.
        # With set_previous=False, the message is treated like a signal, and only the last message will apply.
        step_id = await add_offline_message_to_runtime_outbox(store, references, agent_name, msg, set_previous=False)
        # If this the gnesis step, also create an agen ref
        if(self._is_genesis and self.actor_name is not None):
            # This may override the actor ref, if the genesis step changes over multiple initial pushes, 
            # but once the runtime runs, the actor ref needs to be fix
            await references.set(ref_actor_name(self.actor_name), msg.recipient_id)
        #notify other actors about the creation of this one
        notify_msgs = await self.create_notification_messages(references, msg.recipient_id)
        for notify_msg in notify_msgs:
            await add_offline_message_to_runtime_outbox(store, references, agent_name, notify_msg, set_previous=True)
        return step_id
        
    async def create_actor_message(self, store:ObjectStore) -> OutboxMessage:
        if(self._is_genesis):
            return await self.__create_genesis_message(store)
        else:
            return self.__create_update_message()
        
    async def __create_genesis_message(self, store:ObjectStore) -> OutboxMessage:
        core = self.to_core()
        if not core.has_key("wit"):
            raise InvalidCoreException("Cannot create genesis message without wit.")
        genesis_msg = await OutboxMessage.from_genesis(store, core)
        return genesis_msg
    
    def __create_update_message(self) -> OutboxMessage:
        core = self.to_core()
        if(self._actor_id is None):
            raise Exception("Cannot create update message without actor_id.")
        update_msg = OutboxMessage.from_update(self.actor_id, core)
        return update_msg
    
    async def create_notification_messages(self, references:References, actor_id:ActorId) -> list[OutboxMessage]:
        if(not self._is_genesis):
            return []
        if(len(self.notify) == 0):
            return []
        notify_msgs = []
        for actor_to_notify in self.notify:
            actor_to_notify_id = await references.get(ref_actor_name(actor_to_notify))
            if(actor_to_notify_id is None):
                raise Exception(
                    f"Cannot notify actor {actor_to_notify} because no actor id was found in references '{ref_actor_name(actor_to_notify)}'."
                    )
            notify_msg = OutboxMessage.from_new(actor_to_notify_id, {'actor_name': self.actor_name, 'actor_id': actor_id.hex()})
            notify_msg.mt = "notify_genesis"
            notify_msgs.append(notify_msg)
        return notify_msgs
    

def _blob_from_value(value:any) -> BlobObject:
    if isinstance(value, str):
        return BlobObject.from_str(value)
    elif isinstance(value, bytes):
        return BlobObject.from_bytes(value)
    elif isinstance(value, dict) or isinstance(value, list) or isinstance(value, tuple) or isinstance(value, Iterable):
        return BlobObject.from_json(value)
    else:
        return BlobObject.from_str(str(value))
    
def _blob_from_file(file_path:str) -> BlobObject:
    #todo: check .grit/headers if there are any headers for this file from a previous sync

    # try filetype to guess
    kind = filetype.guess(file_path)
    if kind is not None:
        mime_type = kind.mime
    else:
        mime_type = None
        mimetypes.init()
        mime_type, _ = mimetypes.guess_type(file_path)
        if mime_type is None:
            logger.warn('Cannot guess file mime type: '+file_path)

    headers = {}
    if mime_type is not None:
        if mime_type == 'application/octet-stream':
            headers['ct'] = 'b'
        elif mime_type == 'application/json':
            headers['ct'] = 'j'
        elif mime_type == 'text/plain':
            headers['ct'] = 's'
        elif mime_type == 'text/x-python':
            headers['ct'] = 's'
            headers['Content-Type'] = mime_type
        else:
            headers['Content-Type'] = mime_type
    
    with open(file_path, 'rb') as f:
        bytes = f.read()
    blob = Blob(headers, bytes)
    return BlobObject(blob)


