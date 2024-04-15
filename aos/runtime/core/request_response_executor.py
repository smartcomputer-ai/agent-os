from __future__ import annotations
from grit import *
from wit import *
from .runtime_executor import RuntimeExecutor

#==========================================================================================
# Rails are request-response calls to an actor.
#
# This is to avoid excessive callback hell in certain cases.
# See the documentation in /thinking for more details.
#
# The current implementation just wraps the RuntimeExecutor since it had 
# already everything we needed to implement the first prototype.
# In the future, we will want to make the rails executor its own runtime actor.

class RequestResponseExecutor(RequestResponse):
    def __init__(self, store:ObjectStore, runtime_executor:RuntimeExecutor) -> None:
        self.store = store
        self.runtime_executor = runtime_executor

    async def run(
        self, 
        msg:OutboxMessage, 
        response_types:list[str], 
        timeout:float|None = None,
        ) -> InboxMessage:
        
        if response_types is None or len(response_types) == 0:
            raise Exception("Need at least one response message type to wait for.")
        
        if not msg.is_signal:
            raise Exception("The request 'msg' must be a signal. Set is_signal to True.")

        mailbox_update = await msg.persist_to_mailbox_update(self.store, self.runtime_executor.actor_id)
        with self.runtime_executor.subscribe_to_messages() as queue:
            # send to executor
            await self.runtime_executor.update_current_outbox([mailbox_update])
            # wait for response
            while True:
                if timeout is not None:
                    # the timeout will throw here, if it gets triggered, and bubble up
                    mailbox_update = await asyncio.wait_for(queue.get(), timeout)
                else:
                    mailbox_update = await queue.get()

                if mailbox_update is None:
                    raise Exception("Runtime terminated runtime actor.")
                
                sender_id = mailbox_update[0]
                message_id = mailbox_update[2]
                message = await InboxMessage.from_message_id(self.store, sender_id, message_id)
                
                if message.mt is not None and message.mt in response_types:
                    return message
                    
