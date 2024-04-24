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


     