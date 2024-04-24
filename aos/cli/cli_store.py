import logging
import asyncio
import os
import click
from dataclasses import dataclass
from aos.cli.actor_push import ActorPush
from aos.grit import *
from aos.runtime.store import agent_store_pb2, grit_store_pb2
from aos.runtime.apex import apex_api_pb2
from aos.runtime.web.agents_client import AgentsClient
from . import sync_file as sf
from . import agents_file as af
from .agents_file import Agent

#print logs to console
logging.basicConfig(level=logging.INFO)

# Main CLI to work with agent projects.
# It utilizes the 'click' library.

@dataclass
class StoreContext:
    verbose:bool
    work_dir:str
    aos_dir:str
    agents_file_path:str
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
    agents_file_path = os.path.join(aos_dir, "agents.toml")

    if(verbose):
        print(" work dir: " + work_dir)
        print(" aos dir: " + aos_dir)
        print(" agents file: " + agents_file_path)  

    if(not os.path.exists(work_dir)):
        raise click.ClickException(f"Work directory '{work_dir}' (absolute: '{os.path.abspath(work_dir)}') does not exist.")
    
    ctx.obj = StoreContext(
        verbose=verbose, 
        work_dir=work_dir,
        aos_dir=aos_dir,
        agents_file_path=agents_file_path,
        apex_address=apex_address)
    
#===========================================================
# 'agents' command
#===========================================================
@cli.command()
@click.pass_context
@click.option("--running", "-r", is_flag=True, help="Only running agents.")
def agents(ctx:click.Context, running:bool):
    print("-> Listing Agent")

    store_ctx:StoreContext = ctx.obj

    async def ainit():
        client = AgentsClient(store_ctx.apex_address)

        if not running:
            print("Getting all agents from store...")
            agents = await client.get_agents()
        else:
            print("Getting running agents from apex...")
            agents = await client.get_running_agents()
            
        print(f"{'point':<10} {'agent_id':<20}")
        for agent_id, point in agents.items():
            print(f"{point:<10} {agent_id.hex():<20}")
        

    asyncio.run(ainit())
