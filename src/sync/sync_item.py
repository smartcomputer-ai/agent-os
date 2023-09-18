from __future__ import annotations
import os
import posixpath
from pathlib import PureWindowsPath, PurePosixPath
from attr import dataclass
from grit import *

@dataclass
class SyncItem:
    """An item to sync between the the external world, usually the file system, and Grit."""
    dir_path:str
    file_name:str
    core_path:str
    item_name:str
    item_value:any = None

    @property
    def dir_file_path(self) -> str:
        return os.path.join(self.dir_path, self.file_name)
    @property
    def core_item_path(self) -> str:
        return posixpath.join(self.core_path, self.item_name)
    @property
    def has_item_value(self) -> bool:
        return self.item_value is not None

#===================================================================================================
# Sync items from push paths, files, and values
# Creates grit synchronization candidates to push to an agent later.
#===================================================================================================
def sync_from_push_values(dict:dict[str, any]) -> list[SyncItem]:
    sync_items = []
    for core_path, value in dict.items():
        sync_items.append(sync_from_push_value(core_path, value))
    return sync_items

def sync_from_push_value(core_path:str, value:any) -> SyncItem:
    if(value is None):
        raise ValueError(f"Push value '{value}' must not be None.")
    if(core_path is None or core_path == ""):
        raise ValueError(f"Core path '{core_path}' must not be empty.")
    if(core_path[0] != "/"):
        raise ValueError(f"Core path must start with a slash but was '{core_path}'")
    if(core_path.endswith("/")):
        raise ValueError(f"Core path must not end with a slash, when adding a value, but was '{core_path}'")
    core_path = posixpath.normpath(core_path)
    if(len(core_path) < 2):
        raise ValueError(f"Core path must end with a item/node name, but was '{core_path}'")
    item_name = posixpath.basename(core_path)
    sync_core_path = posixpath.dirname(core_path)
    return SyncItem(dir_path=None, file_name=None, core_path=sync_core_path, item_name=item_name, item_value=value)

def sync_from_push_path(push_path:str, ignore:list[str]=None) -> list[SyncItem]:
    push_path_parts = push_path.split(':')
    if len(push_path_parts) == 1:
        dir_path = push_path_parts[0]
        core_path = ""
    elif len(push_path_parts) == 2:
        dir_path = push_path_parts[0]
        core_path = push_path_parts[1]
    if len(push_path_parts) > 2:
        raise ValueError(f"Push path '{push_path}': must be of the form 'dir:core'.")
    if(dir_path == ""):
        raise ValueError(f"Push path '{push_path}': must contain a directory or file name but was empty.")
    dir_path = os.path.normpath(dir_path)
    if(core_path == ""):
        core_path = "/"
    #core paths always have to start from the root, with a slash
    if(core_path[0] != "/"):
        raise ValueError(f"Push path '{push_path}': Core path must start with a slash but was '{core_path}'")
    #preserve '/' eding in the core path, needed for file copy
    if(core_path.endswith("/") or core_path.endswith("/.")):
        core_path = posixpath.normpath(core_path) + "/"
        if(core_path == "//"):
            core_path = "/"
    else:
        core_path = posixpath.normpath(core_path)

    #now, see if the dir_root is a file or a directory
    if os.path.isdir(dir_path):
        return _sync_from_push_dir_path(dir_path, core_path, ignore)
    #check that the file exists
    elif os.path.isfile(dir_path):
        file_path = dir_path
        sync_dir_path = os.path.dirname(file_path)
        file_name = os.path.basename(file_path)
        # if the core path ends with a slash, then assume the core path is a grit "directory"
        if core_path.endswith("/"):
            item_name = file_name
            sync_core_path = core_path
        #otherwise, assume that the core path is a file path
        else:
            item_name = posixpath.basename(core_path)
            sync_core_path = posixpath.dirname(core_path)
        return [SyncItem(sync_dir_path, file_name, sync_core_path, item_name)]
    else:
        raise ValueError(f"Push path '{push_path}' must contain an existing directory or file.")

def _sync_from_push_dir_path(dir_path:str, core_path:str, ignore:list[str]=None) -> list[SyncItem]:
    '''Descends into directories, except on ignore list. Sorts directories and files by name, and returns a list of SyncItem objects.'''
    if ignore is None:
        ignore = DEFAULT_IGNORE
    # there must be a directory path (TODO: handle '/.', windows, etc)
    if dir_path == "" or dir_path == "/" or dir_path is None:
        raise ValueError('dir_root must contain a directory.')
    # dir_path should end with a '/' for the replacement logic to work
    dir_path = os.path.normpath(dir_path)
    # core_path can be empty, then just assume root
    if core_path is None or core_path == "" or core_path == "//":
        core_path = "/"
    if(core_path[0] != "/"):
        raise ValueError(f"Core path must start with a slash but was '{core_path}'")
    # core_path should end with a '/' or if it is not the root path
    core_path = posixpath.normpath(core_path)
    # now, walk the directory and get all files
    items = []
    for root, dirs, filenames in os.walk(dir_path):
        for filename in sorted(filenames):
            if not _should_ignore(root, filename, ignore):
                sync_dir_path = root
                #replace the directory part of the path to merge it with the core path
                sync_core_path = os.path.relpath(root, dir_path)
                if os.name == "nt":
                    sync_core_path = PureWindowsPath(sync_core_path).as_posix()
                sync_core_path = posixpath.normpath(posixpath.join(core_path, sync_core_path))
                items.append(SyncItem(sync_dir_path, filename, sync_core_path, filename))
        dirs = sorted(dirs)
    for dir in dirs:
        #if a directory is ignored, then don't even descend into it
        # see os.walk documentation on how that works (using topdown=True)
        if _should_ignore(root, dir+"/", ignore):
            del dirs[dir]
    return items

#===================================================================================================
# Sync items to pull from an existing actor core
# Creates synchronization candiates from live actors which can be saved to the local file system
#===================================================================================================
# TODO: WIP


#===================================================================================================
# Utils
#===================================================================================================
DEFAULT_IGNORE = ['.git', '.DS_Store', '.vscode', '__pycache__']

def _should_ignore(root:str, dir_or_file:str, ignore:list[str]) -> bool:
    ''' Check if a file or directory should be ignored. If it's a dir, it should end with a slash'''
    # poor man's implementation of gitignore
    # todo: exapand implementation to be in line with gitignore
    path_name = os.path.join(root, dir_or_file)
    for i in ignore:
        if os.name == "nt" and "/" in i:
            i = i.replace("/", os.sep)
        if i in path_name:
            return True
    return False















