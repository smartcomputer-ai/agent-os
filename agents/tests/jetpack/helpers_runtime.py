from aos.wit import *
from aos.runtime.core import *

async def wait_for_message_type(runtime:Runtime, mt:str) -> Message:
    with runtime.subscribe_to_messages() as queue:
        while True:
            mailbox_update = await queue.get()
            if(mailbox_update is None):
                break
            message_id = mailbox_update[2]
            message:Message = await runtime.store.load(message_id)
            if message.headers is not None and "mt" in message.headers:
                print("test: message received:", message.headers["mt"], message_id.hex())
                if message.headers["mt"] == mt:
                    return message
    print("test: runtime closed subscription.")
    return None