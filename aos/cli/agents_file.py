from dataclasses import dataclass
import os
import tomlkit
from tomlkit import TOMLDocument, table, aot, item
from tomlkit.container import Container


# Functions to work with a remotes.toml file
# Utilizes https://github.com/sdispater/tomlkit to work with TOML data.
#
# The expected toml format is:
# --------------------------
# [[agents]]
# alias = "agent_alias"
# agent_id = "azxzjemxwzxzx" #hex id of the agent
# point = 121231 #point key of the agent
# --------------------------

@dataclass
class Agent:
    alias:str
    agent_id:str
    point:int

def load_agents(toml_file_path:str) -> list[Agent]:
    doc = _read_toml_file(toml_file_path)
    return loads_agents(doc)

def loads_agents(toml:str|TOMLDocument) -> list[Agent]:
    if(isinstance(toml, str)):
        doc = _read_toml_string(toml)
    else:
        doc = toml
    agents = doc.get("agents", None)
    if agents is None:
        return []
    return [Agent(**a) for a in agents]

def add_agent(toml_file_path:str, agent:Agent):
    doc = _read_toml_file(toml_file_path)
    agents = doc.get("agents", None)
    if agents is None:
        agents = aot()
        doc.append("agents", agents)
    else:
        for a in agents:
            if a["alias"] == agent.alias:
                raise Exception(f"Agent with alias '{agent.alias}' already exists.")
    agent_item = item({
        "alias": agent.alias,
        "agent_id": agent.agent_id,
        "point": agent.point
    })
    agents.append(agent_item)
    _write_toml_file(toml_file_path, doc)


def _read_toml_file(file_path) -> TOMLDocument:
    file_path = _convert_posix_to_win(file_path)
    with open(file_path, 'r') as f:
        return _read_toml_string(f.read())

def _read_toml_string(toml_string) -> TOMLDocument:
    return tomlkit.loads(toml_string)

def _write_toml_file(file_path, doc:TOMLDocument):
    file_path = _convert_posix_to_win(file_path)
    with open(file_path, 'w') as f:
        f.write(doc.as_string())

def _convert_posix_to_win(path:str) -> str:
    if os.name == "nt" and "/" in path:
        return path.replace("/", os.sep)
    return path
