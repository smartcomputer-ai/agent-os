
from aos.runtime.apex import apex_pb2

import logging

# Steps:
# 0) get this node id
# 1) get all agents and their actors from grit
# 2) gather unprocessed messages (between actors)
# 3) wait for workers
# 4) assign actors to workers (compare actor's wit manifest to worker's capabilities)
# 5) send messages to workers 

# if new actor: make sure if it is a genesis or update message that the worker can handle the message)

async def core_loop():
    pass

