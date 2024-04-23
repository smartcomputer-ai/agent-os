import logging
import asyncio
import os
import shutil
import sys
import click
from dataclasses import dataclass
from aos.grit import *
from aos.runtime.core.root_executor import create_or_load_root_actor
from .actor_push import ActorPush
from . import sync_file as sf

#print logs to console
logging.basicConfig(level=logging.INFO)

# Main CLI to work with agent projects.
# It utilizes the 'click' library.

@dataclass
class LocalContext:
    verbose:bool
    work_dir:str
    sync_file_path:str

    def enforce_paths_exist(self):
        if not os.path.exists(self.work_dir):
            raise click.ClickException(f"work directory '{self.work_dir}' does not exist.")
        if not os.path.exists(self.sync_file_path):
            raise click.ClickException(f"sync file '{self.sync_file_path}' does not exist.")
        
@click.group()
@click.pass_context
@click.option("--work-dir", "-d", help="Work directory. By default, uses the current directory. All other files and paths will be relative to this.")
@click.option("--verbose", "-v", is_flag=True, help="Will print verbose messages.")
def cli(ctx:click.Context, verbose:bool, work_dir:str|None):
    if(work_dir is None):
        work_dir = os.getcwd()
    sync_file = "sync.toml"
    sync_file_path = os.path.join(work_dir, sync_file)

    if(verbose):
        print(" work_dir: " + work_dir)
        print(" sync_file: " + sync_file_path)  

    if(not os.path.exists(work_dir)):
        raise click.ClickException(f"Work directory '{work_dir}' (absolute: '{os.path.abspath(work_dir)}') does not exist.")
    ctx.obj = LocalContext(
        verbose=verbose, 
        work_dir=work_dir,
        sync_file_path=sync_file_path,)
    
#===========================================================
# 'init' command
#===========================================================
@cli.command()
@click.pass_context
def init(ctx:click.Context):
    print("-> Initializing local env")
    local_ctx:LocalContext = ctx.obj

    sync_file = f"""
# [[actors]]
# name = "example_name" #optional
# push = "path/to/wit_one:/code" #is merged with all, can be ommitted, then the all sync is used
# wit = "/code:module:function_name" 
# wit_query = "/code:module:function_name" 
"""

    if(not os.path.exists(local_ctx.sync_file_path)):
        with open(local_ctx.sync_file_path, "w") as f:
            f.write(sync_file)
        print("Sync file initialized: " + local_ctx.sync_file_path)
    else:
        print("Sync file already exists: " + local_ctx.sync_file_path)

    #sanity check that all needed paths exist
    local_ctx.enforce_paths_exist()


#===========================================================
# 'reset' command
#===========================================================
@cli.command()
@click.pass_context
@click.option("--no-push", is_flag=True, help="Only delete grit, do not push.")
def reset(ctx:click.Context, no_push:bool):
    print("-> Resetting Agent")
    local_ctx:LocalContext = ctx.obj
    
    if(not os.path.exists(local_ctx.sync_file_path)):
        raise click.ClickException(f"Sync file '{local_ctx.sync_file_path}' does not exist, assuming this is not an existing agent.")

    print("not implemented, NO OP.")
    # if(os.path.exists(wit_ctx.grit_dir)):
    #     print("Deleting grit directory: " + wit_ctx.grit_dir)
    #     shutil.rmtree(wit_ctx.grit_dir)
    #     os.makedirs(wit_ctx.grit_dir, exist_ok=True)
    #     print("Created fresh grit directory: " + wit_ctx.grit_dir)

    #sanity check that all needed paths exist
    local_ctx.enforce_paths_exist()

    if(not no_push):
        ctx.invoke(push)

#===========================================================
# 'push' command
#===========================================================
@cli.command()
@click.pass_context
def push(ctx:click.Context):
    print("-> Pushing Files to Actors")
    wit_ctx:LocalContext = ctx.obj

    if not os.path.exists(wit_ctx.sync_file_path):
        raise click.ClickException(f"Sync file '{wit_ctx.sync_file_path}' does not exist, can only push from existing sync file, init file with 'aos local init'.")

    wit_ctx.enforce_paths_exist()

    agent_config = sf.load_agent(wit_ctx.sync_file_path)
    agent_name = agent_config["name"]
    print("Agent name: " + agent_name)

    async def apush():
        #check that agent exists
        #create client

        print("TODO, NO OP")
        # store, refs = wit_ctx.init_stores()
        
        # pushes = await sf.load_pushes(wit_ctx.sync_file_path, refs)
        # if(len(pushes) == 0):
        #     print(f"Nothing to push to actos. Define some actors under '[[actors]]' headings in the sync file: {wit_ctx.sync_file_path}")
        #     return
        
        # pushes_to_apply:list[ActorPush] = []
        # for push in pushes:
        #     print(f"Diff of what to push to actor '{push.actor_name}' with id '{push.actor_id.hex() if not push.is_genesis else 'genesis'}'")
        #     apply = False
        #     async for path, reason in push.diff_core_with_actor(store, refs):
        #         print(f"  Diff '{path}' because '{reason}'")
        #         apply = True
        #     if(apply):
        #         pushes_to_apply.append(push)
        #     else:
        #         print("  No changes to push.")
        # if(len(pushes_to_apply) == 0):
        #     print("No pushes to actors.")
        #     return
        
        # for push in pushes_to_apply:
        #     print(f"Pushing to actor: {push.actor_name} '{push.actor_id.hex() if not push.is_genesis else 'genesis'}'")
        #     agent_step_id = await push.create_and_inject_messages(store, refs, agent_name)
        #     print(f"  New agent step id for this push: {agent_step_id.hex()}")        

    asyncio.run(apush())
     