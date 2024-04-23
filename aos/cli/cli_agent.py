import logging
import asyncio
import os
import click
from dataclasses import dataclass
from aos.grit import *
from aos.runtime.store import agent_store_pb2, grit_store_pb2
from aos.runtime.apex import apex_api_pb2
from aos.runtime.web.agents_client import AgentsClient
from . import agents_file as af
from .agents_file import Agent

#print logs to console
logging.basicConfig(level=logging.INFO)

# Main CLI to work with agent projects.
# It utilizes the 'click' library.

@dataclass
class AosContext:
    verbose:bool
    work_dir:str
    aos_dir:str
    agents_file_path:str
    sync_file_path:str
    apex_address:str

    def enforce_paths_exist(self):
        if not os.path.exists(self.work_dir):
            raise click.ClickException(f"work directory '{self.work_dir}' does not exist.")
        if not os.path.exists(self.aos_dir):
            raise click.ClickException(f"aos directory '{self.aos_dir}' does not exist.")
        if not os.path.exists(self.agents_file_path):
            raise click.ClickException(f"agents file '{self.agents_file_path}' does not exist.")
        
@click.group()
@click.pass_context
@click.option("--work-dir", "-d", help="Work directory. By default, uses the current directory. All other files and paths will be relative to this.")
@click.option("--apex-address", required=False, default="localhost:50052", help="Address of the apex server.")
@click.option("--verbose", "-v", is_flag=True, help="Will print verbose messages.")
def cli(ctx:click.Context, verbose:bool, work_dir:str|None, apex_address:str):
    if(work_dir is None):
        work_dir = os.getcwd()
    aos_dir = os.path.join(work_dir, ".aos")
    sync_file_path = os.path.join(work_dir, "sync.toml")
    agents_file_path = os.path.join(aos_dir, "agents.toml")

    if(verbose):
        print(" work dir: " + work_dir)
        print(" aos dir: " + aos_dir)
        print(" agents file: " + agents_file_path)  
        print(" sync file: " + sync_file_path)  

    if(not os.path.exists(work_dir)):
        raise click.ClickException(f"Work directory '{work_dir}' (absolute: '{os.path.abspath(work_dir)}') does not exist.")
    
    ctx.obj = AosContext(
        verbose=verbose, 
        work_dir=work_dir,
        aos_dir=aos_dir,
        agents_file_path=agents_file_path,
        sync_file_path=sync_file_path,
        apex_address=apex_address)
    
#===========================================================
# 'init' command
#===========================================================
@cli.command()
@click.pass_context
@click.option("--agent-alias", "-a", required=True, help="Agent alias which acts as a local reference.")
def create(ctx:click.Context, agent_alias:str):
    print("-> Creating Agent")

    aos_ctx:AosContext = ctx.obj

    if(not os.path.exists(aos_ctx.aos_dir)):
        os.makedirs(aos_ctx.aos_dir, exist_ok=True)
        print("Created aos directory: " + aos_ctx.aos_dir)

    if(not os.path.exists(aos_ctx.agents_file_path)):
        with open(aos_ctx.agents_file_path, "w") as f:
            f.write("")
        print("Agents file initialized: " + aos_ctx.agents_file_path)

    #sanity check that all needed paths exist
    aos_ctx.enforce_paths_exist()

    #check if the alias already exists
    agents = af.load_agents(aos_ctx.agents_file_path)

    if(any(a.alias == agent_alias for a in agents)):
        raise click.ClickException(f"Agent with alias '{agent_alias}' already exists in agent file {aos_ctx.agents_file_path}.")

    async def ainit():
        client = AgentsClient(aos_ctx.apex_address)

        #create the agent
        store_client = await client._get_store_client()
        agent_response:agent_store_pb2.CreateAgentResponse = await store_client.get_agent_store_stub_async().CreateAgent(agent_store_pb2.CreateAgentRequest())
        agent = Agent(alias=agent_alias, agent_id=agent_response.agent_id.hex(), agent_did=agent_response.agent_did)
        af.add_agent(aos_ctx.agents_file_path, agent)
        print(f"Created agent with alias '{agent_alias}' and id '{agent_response.agent_id.hex()}' and did '{agent_response.agent_did}'")

        #start the agent
        apex_client = await client._get_apex_client()
        await apex_client.get_apex_api_stub_async().StartAgent(apex_api_pb2.StartAgentRequest(agent_id=agent_response.agent_id))
        print(f"Started agent with alias '{agent_alias}' and id '{agent_response.agent_id.hex()}'")

    asyncio.run(ainit())

#===========================================================
# 'Push' command
#===========================================================
@cli.command()
@click.pass_context
@click.option("--agent-alias", "-a", required=True, help="Agent alias to push to.")
def push(ctx:click.Context, agent_alias:str):
    print("-> Push to Agent")
    aos_ctx:AosContext = ctx.obj

    #sanity check that all needed paths exist
    aos_ctx.enforce_paths_exist()

    #find the agent 
    agents = af.load_agents(aos_ctx.agents_file_path)
    agent = next((a for a in agents if a.alias == agent_alias), None)
    if(agent is None):
        raise click.ClickException(f"Agent with alias '{agent_alias}' does not exist in agent file {aos_ctx.agents_file_path}.")
    
    async def ainit():
        pass

    asyncio.run(ainit())

#===========================================================
# TODO: start, stop, reset/delete, list, etc
#===========================================================