import asyncio
from dataclasses import dataclass
import os
import sys
import click
from grit import *
from grit.stores.file import FileObjectStore, FileReferences
from grit.stores.lmdb import SharedEnvironment, LmdbObjectStore, LmdbReferences
from grit.stores.memory import MemoryObjectStore, MemoryReferences
from runtime.runtime_executor import create_or_load_runtime_actor
from runtime.runtime import Runtime
from sync.actor_push import ActorPush
from sync.sync_item import SyncItem
from web.web_server import WebServer
import sync.sync_file as sf

# Main CLI to work with agent projects.
# It utilizes the 'click' library.

@dataclass
class WitContext:
    verbose:bool
    work_dir:str
    grit_dir:str
    sync_file:str
    sync_file_path:str
    store_type:str

    def init_stores(self) -> tuple[ObjectStore, References]:
        if(self.store_type == "lmdb"):
            #check if .grit has been initialized with 'file' store
            if os.path.exists(os.path.join(self.grit_dir, "obj")) or os.path.exists(os.path.join(self.grit_dir, "refs")):
                raise click.ClickException(f"grit directory '{self.grit_dir}' has already been initialized with a 'file' store. Cannot use 'lmdb' store.")
            lmdb_env = SharedEnvironment(self.grit_dir, writemap=True)
            return LmdbObjectStore(lmdb_env), LmdbReferences(lmdb_env)
        elif(self.store_type == "file"):
            #check if .grit has been initialized with 'lmdb' store
            if os.path.exists(os.path.join(self.grit_dir, "data.mdb")) or os.path.exists(os.path.join(self.grit_dir, "lock.mdb")):
                raise click.ClickException(f"grit directory '{self.grit_dir}' has already been initialized with a 'lmdb' store. Cannot use 'file' store.")
            return FileObjectStore(self.grit_dir), FileReferences(self.grit_dir)
        elif(self.store_type == "memory"):
            return MemoryObjectStore(), MemoryReferences()
        else:
            raise Exception(f"Unknown store type '{self.store_type}'.")

    def enforce_paths_exist(self):
        if not os.path.exists(self.work_dir):
            raise click.ClickException(f"work directory '{self.work_dir}' does not exist.")
        if not os.path.exists(self.grit_dir):
            raise click.ClickException(f"grit directory '{self.grit_dir}' does not exist.")
        if not os.path.exists(self.sync_file_path):
            raise click.ClickException(f"sync file '{self.sync_file_path}' does not exist.")

@click.group()
@click.pass_context
@click.option("--work-dir", "-d", help="Work directory. By default, uses the current directory. All other files and paths will be relative to this.")
@click.option("--sync-file", "-s", show_default=True, default="sync.toml", help="What sync file to use, if not the default one.")
@click.option("--store-type", default="lmdb", show_default=True, type=click.Choice(['lmdb', 'file', 'memory'], case_sensitive=False), help="What type of object store to use.")
@click.option("--verbose", "-v", is_flag=True, help="Will print verbose messages.")
def cli(ctx:click.Context, verbose:bool, work_dir:str|None, sync_file:str, store_type:str):
    if(work_dir is None):
        work_dir = os.getcwd()
    grit = ".grit"
    grit_dir = os.path.join(work_dir, grit)
    print(" work_dir: " + work_dir)  
    print(" grit_dir: " + grit_dir)    
    print(" sync_file: " + sync_file)  
    if(not os.path.exists(work_dir)):
        raise click.ClickException(f"Work directory '{work_dir}' (absolute: '{os.path.abspath(work_dir)}') does not exist.")
    ctx.obj = WitContext(
        verbose=verbose, 
        work_dir=work_dir, 
        grit_dir=grit_dir, 
        sync_file=sync_file, 
        sync_file_path=os.path.join(work_dir, sync_file), 
        store_type=store_type)
    

#===========================================================
# 'init' command
#===========================================================
@cli.command()
@click.pass_context
@click.option("--agent-name", "-n", required=True, help="Agent reference name. Used to identify the agent in the runtime and generate the agent id.")
def init(ctx:click.Context, agent_name:str):
    print("-> Initializing Agent")
    wit_ctx:WitContext = ctx.obj
    print("Agent name: " + agent_name)
    
    if(not os.path.exists(wit_ctx.grit_dir)):
        os.makedirs(wit_ctx.grit_dir, exist_ok=True)
        print("Created grit directory: " + wit_ctx.grit_dir)

    if(not os.path.exists(wit_ctx.sync_file_path)):
        file_contents = f'''[agent]
name = "{agent_name}"
'''
        with open(wit_ctx.sync_file_path, "w") as f:
            f.write(file_contents)
        print("Sync file initialized: " + wit_ctx.sync_file_path)
    else:
        print("Sync file already exists: " + wit_ctx.sync_file_path)

    #sanity check that all needed paths exist
    wit_ctx.enforce_paths_exist()

    async def ainit():
        store, refs = wit_ctx.init_stores()
        agent_id, step_id = await create_or_load_runtime_actor(store, refs, agent_name)
        print("Agent id: " + agent_id.hex())
        print("Last step id: " + step_id.hex())
    asyncio.run(ainit())

#===========================================================
# 'push' command
#===========================================================
@cli.command()
@click.pass_context
def push(ctx:click.Context):
    print("-> Pushing Files to Actors")
    wit_ctx:WitContext = ctx.obj

    #the user might have deleted the grit directory, create it again (but only if the sync file exists, indicating it has been inintialized before)
    if(os.path.exists(wit_ctx.sync_file_path) and not os.path.exists(wit_ctx.grit_dir)):
        os.makedirs(wit_ctx.grit_dir, exist_ok=True)
        print("Created grit directory: " + wit_ctx.grit_dir)

    wit_ctx.enforce_paths_exist()

    agent_config = sf.load_agent(wit_ctx.sync_file_path)
    agent_name = agent_config["name"]
    print("Agent name: " + agent_name)

    async def apush():
        store, refs = wit_ctx.init_stores()
        
        pushes = await sf.load_pushes(wit_ctx.sync_file_path, refs)
        if(len(pushes) == 0):
            print(f"Nothing to push to actos. Define some actors under '[[actors]]' headings in the sync file: {wit_ctx.sync_file_path}")
            return
        
        pushes_to_apply:list[ActorPush] = []
        for push in pushes:
            print(f"Diff of what to push to actor '{push.actor_name}' with id '{push.actor_id.hex() if not push.is_genesis else 'genesis'}'")
            apply = False
            async for path, reason in push.diff_core_with_actor(store, refs):
                print(f"  Diff '{path}' because '{reason}'")
                apply = True
            if(apply):
                pushes_to_apply.append(push)
            else:
                print("  No changes to push.")
        if(len(pushes_to_apply) == 0):
            print("No pushes to actors.")
            return
        
        for push in pushes_to_apply:
            print(f"Pushing to actor: {push.actor_name} '{push.actor_id.hex() if not push.is_genesis else 'genesis'}'")
            agent_step_id = await push.create_and_inject_messages(store, refs, agent_name)
            print(f"  New agent step id for this push: {agent_step_id.hex()}")        

    asyncio.run(apush())
     

#===========================================================
# 'run' command
#===========================================================
@cli.command()
@click.pass_context
def run(ctx:click.Context):
    print("-> Running Agent")
    wit_ctx:WitContext = ctx.obj
    wit_ctx.enforce_paths_exist()

    agent_config = sf.load_agent(wit_ctx.sync_file_path)
    agent_name = agent_config["name"]
    print("Agent name: " + agent_name)

    async def arun():
        print("Grit dir: "+wit_ctx.grit_dir)
        store, refs = wit_ctx.init_stores()

        #load the agents and register the external paths
        pushes = await sf.load_pushes(wit_ctx.sync_file_path, refs)
        for push in pushes:
            _try_add_to_path(push)

        runtime = Runtime(store, refs, agent_name)
        print("Agent id: "+runtime.agent_id.hex())
        runtime_task = asyncio.create_task(runtime.start())
        print("Runtime starting...")
        await runtime.wait_until_running()
        print("Runtime started")
        actors = runtime.get_actors()
        print(f"Actors: {len(actors)}")
        if(len(actors) == 0):
            print(f"WARNING: no actors available in the runtime!")

        web_sever = WebServer(runtime)
        web_task = asyncio.create_task(web_sever.run())
        print("Web server started")

        await asyncio.wait([runtime_task, web_task], return_when=asyncio.FIRST_COMPLETED)
        await web_task
        web_task.result()
        print("Web server stopped")
        runtime.stop()
        await runtime_task
        print("Runtime stopped")
        web_sever.stop()

    asyncio.run(arun())

def _try_add_to_path(actor_push:ActorPush):
    add_paths = False
    modules = []
    if(actor_push.wit is not None and actor_push.wit.startswith("external:")):
        wit_ref = actor_push.wit[9:]
        ref_parts = wit_ref.split(":")
        if(len(ref_parts) > 2):
            raise Exception(f"Invalid external 'wit' reference: {wit_ref}")
        elif(len(ref_parts) == 2):
            modules.append(ref_parts[0])
            add_paths = True
    if(actor_push.wit_query is not None and actor_push.wit_query.startswith("external:")):
        query_ref = actor_push.wit_query[9:]
        ref_parts = query_ref.split(":")
        if(len(ref_parts) > 2):
            raise Exception(f"Invalid external 'wit_query' reference: {query_ref}")
        elif(len(ref_parts) == 2):
            modules.append(ref_parts[0])
            add_paths = True

    if(add_paths):
        search_paths = set()
        for si in actor_push.sync_items:
            if(si.file_name is not None and si.file_name.endswith(".py")):
                search_paths.add(si.dir_path)
                #also add the parent directory, so that packages can be imported too
                search_paths.add(os.path.normpath(os.path.join(si.dir_path, "..")))
        for search_path in search_paths:
            if(not os.path.exists(search_path)):
                raise Exception(f"Search path does not exist: {search_path}")
            #if the path contains an __init__.py file, then it is a package and we should not add the path
            if(os.path.exists(os.path.join(search_path, "__init__.py"))):
                print(f"Skipping search path because it is a package: {search_path}")
                continue
            if(search_path not in sys.path):
                print(f"Adding search path: {search_path}")
                sys.path.append(search_path)
            #also see if the module has been loaded previously and reload it now (for watch mode)
            # for module in modules:
            #     if(module in sys.modules):
            #         importlib.reload(sys.modules[module])


#===========================================================
# Utils
#===========================================================
def print_grit_file_stats(grit_dir):
    files = os.listdir(grit_dir)
    file_bytes = 0
    for root, dirs, files in os.walk(grit_dir):
        for file in files:
            file_bytes += os.path.getsize(os.path.join(root,file))
    file_bytes = file_bytes / 1024 / 1024
    print(f"Temp dir {grit_dir} has {len(files)} files, and is {file_bytes:0.2f} MB")


if __name__ == '__main__':
    cli(None)