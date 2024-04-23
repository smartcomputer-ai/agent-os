from aos.grit import *
from aos.wit import *

empty = Wit()

@empty.run_wit
async def empty_wit(inbox:Inbox, outbox:Outbox, core:Core, **kwargs):
    messages = await inbox.read_new()
    for message in messages:
        print("Wit message:", message.sender_id.hex(), message.mt)

#TODO: add more utility wits here
