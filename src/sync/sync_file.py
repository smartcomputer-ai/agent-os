import os
import tomlkit
from tomlkit import TOMLDocument
from tomlkit.container import Container
from grit import *
from .actor_push import ActorPush

# Functions to work with a sync.toml file
# Utilizes https://github.com/sdispater/tomlkit to work with TOML data.
#
# The expected toml format is:
# --------------------------
# [agent]
# name = "agent_name" #optional

# [all] #optional
# #push can also be a json (or inline table in TOML)
# push = "path/to/common_code:/"
# push_values = { 
#     "/data/args": "hello world",
#     "/data/more_args": {"hello": "world"},
# }
# push_on_genesis = "path/to/common_code:/"
# pull = "path/to/common_code:/"
# sync = "path/to/common_code:/" supports both push and pull
# external_paths = ["path/to/common_code", "path/to/other_code"] #optional

# [[actors]]
# name = "actor_one" #optional
# push = "path/to/wit_one:/code" #is merged with all, can be ommitted, then the all sync is used
# wit = "/code:module:function_name" 
# wit_query = "/code:module:function_name" 

# [[actors]]
# name = "actor_two"
# push = "path/to/wit_two" 
# wit = "external:module:function_name" 
# wit_query = "external:module:function_name" 
# runtime = "python" #which runtime to use, default is python
# --------------------------

def load_agent(toml_file_path:str) -> dict[str,str]:
    doc = _read_toml_file(toml_file_path)
    return loads_agent(doc)

def loads_agent(toml:str|TOMLDocument) -> dict[str,str]:
    if(isinstance(toml, str)):
        doc = _read_toml_string(toml)
    else:
        doc = toml
    _validate_doc(doc)
    agent = doc["agent"]
    return dict(agent)

def load_paths(toml_file_path:str) -> dict[str,str]:
    doc = _read_toml_file(toml_file_path)
    return loads_paths(doc)

def loads_paths(toml:str|TOMLDocument) -> list[str]:
    if(isinstance(toml, str)):
        doc = _read_toml_string(toml)
    else:
        doc = toml
    _validate_doc(doc)
    paths = []
    all = doc.get("all", None)
    if(all is not None and 'external_paths' in all):
        all_paths = all['external_paths']
        if(isinstance(all_paths, str)):
            paths.append(all_paths)
        elif(isinstance(all_paths, list)):
            paths.extend(all_paths)
    
    actors = doc.get("actors", None)
    if(actors is not None):
        for actor_table in actors:
            if 'external_paths' in actor_table:
                actor_paths = actor_table['external_paths']
                if(isinstance(actor_paths, str)):
                    paths.append(actor_paths)
                elif(isinstance(actor_paths, list)):
                    paths.extend(actor_paths)
    return paths

async def load_pushes(toml_file_path:str, references:References) -> list[ActorPush]:
    doc = _read_toml_file(toml_file_path)
    return await loads_pushes(doc, references)

async def loads_pushes(toml:str|TOMLDocument, references:References) -> list[ActorPush]:
    if(isinstance(toml, str)):
        doc = _read_toml_string(toml)
    else:
        doc = toml
    _validate_doc(doc)
    all = doc.get("all", None)
    actors = doc.get("actors", None)
    if(actors is None):
        return []
    actor_pushes = []
    for actor_table in actors:
        actor_push = await _actor_push_from_toml_table(references, actor_table, all)
        actor_pushes.append(actor_push)
    return actor_pushes

async def _actor_push_from_toml_table(references:References, actor_table:Container, all:Container|None=None) -> ActorPush:        
    actor_name = actor_table['name']
    actor_push = await ActorPush.from_actor_name(references, actor_name)
    if all is not None:
        _add_table_to_actor_push(actor_push, all)
    _add_table_to_actor_push(actor_push, actor_table)
    return actor_push

def _add_table_to_actor_push(actor_push:ActorPush, table:Container):
    def try_add_path(path_key:str):
        paths = table.get(path_key, None)
        if paths is not None:
            if(isinstance(paths, str)):
                paths = [paths]
            elif(not isinstance(paths, list)):
                raise ValueError(f"Table item '{path_key}' must be a string or list of strings, but was {type(paths)}.")
            for path in paths:
                actor_push.add_push_path(path)
    def try_add_values(values_key:str):
        values:Container = table.get(values_key, None)
        if values is not None:
            for key, value in values.items():
                actor_push.add_push_value(key, value, actor_push.is_genesis)
    #iterate through the keys in the table and add them in the order they appear
    for key in table.keys():
        if key in ["push", "sync"]:
            try_add_path(key)
        elif key == "push_on_genesis" and actor_push.is_genesis:
            try_add_path(key)
        elif key == "push_value":
            try_add_values(key)
        elif key == "push_value_on_genesis" and actor_push.is_genesis:
            try_add_values(key)
    if "wit" in table:
        actor_push.wit = table["wit"]
    if "wit_query" in table:
        actor_push.wit_query = table["wit_query"]
    if "wit_update" in table and not actor_push.is_genesis:
        actor_push.wit_update = table["wit_update"]
    if "name" in table:
        actor_push.actor_name = table["name"]
    if "notify" in table:
        notify = table['notify']
        if(isinstance(notify, str)):
            notify = [notify]
        for actor_to_notify in notify:
            actor_push.notify.add(actor_to_notify)

def _read_toml_file(file_path) -> TOMLDocument:
    file_path = _convert_posix_to_win(file_path)
    with open(file_path, 'r') as f:
        return _read_toml_string(f.read())

def _read_toml_string(toml_string) -> TOMLDocument:
    return tomlkit.loads(toml_string)

def _validate_doc(doc:tomlkit.TOMLDocument) -> None:
    valid_top_level_keys = ['agent', 'all', 'actors']
    for key in doc.keys():
        if key not in valid_top_level_keys:
            raise ValueError(f"Invalid top level key '{key}'. Valid keys are '{valid_top_level_keys}'.")

    def validate_agent(agent):
        if not agent.is_table():
            raise ValueError("The agent table is not a table. Use [agent] to define the agent.")
        valid_agent_keys = ['name']
        for key in agent.keys():
            if key not in valid_agent_keys:
                raise ValueError(f"Invalid agent key '{key}'. Valid keys are '{valid_agent_keys}'.")

    def validate_all(all, valid_keys):
        if not all.is_table():
            raise ValueError("The all table is not a table. Use [all] to define the all table.")
        for key in all.keys():
            if key not in valid_keys:
                raise ValueError(f"Invalid all key '{key}'. Valid keys are '{valid_keys}'.")

    def validate_actors(actors, valid_keys):
        if not actors.is_aot():
            raise ValueError("The actors table array (or heading) is not an array of tables. Use [[actors]] to define multiple actors.")
        actor_names = []
        for actor in actors:
            for key in actor.keys():
                if key not in valid_keys:
                    raise ValueError(f"Invalid actor key '{key}'. Valid keys are '{valid_keys}'.")
            if 'name' not in actor:
                raise ValueError("Actor name is required. Use 'name' to define an actor reference name.")
            else:
                actor_names.append(actor['name'])
            #check that the actors that need to be notified about this actor have been defined *before* this actor
            if 'notify' in actor:
                notify = actor['notify']
                if(isinstance(notify, str)):
                    notify = [notify]
                if(not isinstance(notify, list)):
                    raise ValueError(f"Actor to notify must be a string or list of strings, but was {type(notify)}.")
                for actor_to_notify in notify:
                    if(actor_to_notify not in actor_names):
                        raise ValueError(f"Actor to notify '{actor_to_notify}' has not been defined before this one '{actor['name']}'. "+
                                         f"Define the actor '{actor_to_notify}' above this one.")

    if('agent' in doc):
        validate_agent(doc['agent'])
    valid_all_keys = ['push', 'pull', 'sync', 'push_value', 'push_on_genesis', 'push_value_on_genesis', 'runtime', 'external_paths']
    if('all' in doc):
        validate_all(doc['all'], valid_all_keys)
    valid_actor_keys = ['name', 'wit', 'wit_query', 'notify'] + valid_all_keys
    if 'actors' in doc:
        #raise ValueError('No actors table array (aka heading) is defined. Use [[actors]] to define one or more actors.')
        validate_actors(doc['actors'], valid_actor_keys)

def _convert_posix_to_win(path:str) -> str:
    if os.name == "nt" and "/" in path:
        return path.replace("/", os.sep)
    return path
