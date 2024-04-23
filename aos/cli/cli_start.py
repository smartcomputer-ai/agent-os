import logging
import asyncio
import os
import shutil
import sys
import click
from dataclasses import dataclass
from aos.grit import *
from aos.runtime.store.store_server import start_server_sync as start_store_server_sync, start_server as start_store_server
from aos.runtime.apex.apex_server import start_server as start_apex_server
from aos.runtime.worker.worker_server import start_server as start_worker_server
from aos.runtime.web.web_server import WebServer
from aos.runtime.web.agents_client import AgentsClient

#print logs to console
logging.basicConfig(level=logging.INFO)

@dataclass
class ClusterContext:
    verbose:bool
        
@click.group()
@click.pass_context
@click.option("--verbose", "-v", is_flag=True, help="Will print verbose messages.")
def cli(ctx:click.Context, verbose:bool):
    ctx.obj = ClusterContext(
        verbose=verbose,
    )

#===========================================================
# 'store' command
#===========================================================
@cli.command(context_settings={'show_default': True})
@click.pass_context
@click.option("--store-dir", "-d", required=True, help="Where to store the data.")
@click.option("--port", "-p", required=False, default=50051, help="Port of the store server.")
def store(ctx:click.Context, port:int, store_dir:str,):
    """Starts the store server. There should be only one store server."""
    print("-> Starting Store Server")

    start_store_server_sync(grit_dir=store_dir, port=str(port))

#===========================================================
# 'apex' command
#===========================================================
@cli.command(context_settings={'show_default': True})
@click.pass_context
@click.option("--port", "-p", required=False, default=50052, help="Port of the apex server.")
@click.option("--store-address", required=False, default="localhost:50051", help="Address of the store server.")
def apex(ctx:click.Context, port:int, store_address:str):
    """Starts the apex server. There should be only one apex server."""
    print("-> Starting Apex Server")

    async def ainit():
        await start_apex_server(store_address=store_address, port=str(port))
    
    asyncio.run(ainit())


#===========================================================
# 'worker' command
#===========================================================
@cli.command(context_settings={'show_default': True})
@click.pass_context
@click.option("--port", "-p", required=False, default=50053, help="Port of the worker server.")
@click.option("--store-address", required=False, default="localhost:50051", help="Address of the store server.")
@click.option("--apex-address", required=False, default="localhost:50052", help="Address of the apex server.")
@click.option("--worker-address", required=False, default=None, help="Address of this worker where others can reach it at, will be broadcasted to others via apex. If none is provided, will be set to localhost:<port>.")
@click.option("--worker-id", required=False, default=None, help="The permanent identity of this worker. If not provided, an emphemeral id will be generated.")
def worker(ctx:click.Context, port:int, store_address:str, apex_address:str, worker_address:str, worker_id:str|None):
    """Starts a worker server. More than one worker server can be started, but the port and worker address must be different."""
    print("-> Starting Worker Server")

    if worker_address is None:
        worker_address = "localhost:" + str(port)

    async def ainit():
        await start_worker_server(
            store_address=store_address,
            apex_address=apex_address,
            port=str(port),
            worker_address=worker_address,
            worker_id=worker_id
            )
    
    asyncio.run(ainit())


#===========================================================
# 'web' command
#===========================================================
@cli.command(context_settings={'show_default': True})
@click.pass_context
@click.option("--port", "-p", required=False, default=5000, help="Port of the web server.")
@click.option("--apex-address", required=False, default="localhost:50052", help="Address of the apex server.")
def web(ctx:click.Context, port:int, apex_address:str):
    """Starts the web server. There can be multiple web servers. A web server must be able to connect to the apex and store server."""
    print("-> Starting Web Server")

    async def ainit():
        web_server = WebServer(AgentsClient(apex_address=apex_address))
        await web_server.run(port=port)
    
    asyncio.run(ainit())


#===========================================================
# 'all' command
#===========================================================
@cli.command(context_settings={'show_default': True})
@click.option("--store-dir", "-d", required=True, help="Where to store the data.")
@click.pass_context
def all(ctx:click.Context, store_dir:str):
    """Starts all servers in the same process, best for testing. Uses the default ports and addresses."""
    print("-> Starting All Servers")

    async def ainit():
        print("--> Starting Store Server")
        store_server_task = asyncio.create_task(start_store_server(grit_dir=store_dir))
        print("--> Starting Apex Server")
        apex_server_task = asyncio.create_task(start_apex_server())
        print("--> Starting Worker Server")
        worker_server_task = asyncio.create_task(start_worker_server())
        print("--> Starting Web Server")
        web_server_task = asyncio.create_task(WebServer(AgentsClient()).run())

        await asyncio.wait([store_server_task, apex_server_task, worker_server_task, web_server_task], return_when=asyncio.FIRST_COMPLETED)
        #cancel the rest
        store_server_task.cancel()
        apex_server_task.cancel()
        worker_server_task.cancel()
        web_server_task.cancel()

    asyncio.run(ainit())


# def _add_to_path(sync_file:str):
#     external_paths = sf.load_paths(sync_file)
#     for external_path in external_paths:
#         if external_path not in sys.path:
#             print(f"Adding search path: {external_path}")
#             sys.path.append(external_path)

#===========================================================
# Utils
#===========================================================
def print_grit_file_stats(grit_dir):
    files = os.listdir(grit_dir)
    file_bytes = 0
    for root, _, files in os.walk(grit_dir):
        for file in files:
            file_bytes += os.path.getsize(os.path.join(root,file))
    file_bytes = file_bytes / 1024 / 1024
    print(f"Temp dir {grit_dir} has {len(files)} files, and is {file_bytes:0.2f} MB")