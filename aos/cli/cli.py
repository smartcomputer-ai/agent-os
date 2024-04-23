import logging
import asyncio
import os
import shutil
import sys
import click
from .cli_local import cli as cli_local
from .cli_start import cli as cli_start
from .cli_agent import cli as cli_agent

#print logs to console
logging.basicConfig(level=logging.INFO)

# Main CLI to work with agent projects.
# It utilizes the 'click' library.


@click.group()
def cli():
    pass

cli.add_command(cli_local, name="local")
cli.add_command(cli_start, name="start")
cli.add_command(cli_agent, name="agent")

if __name__ == '__main__':
    cli(None)