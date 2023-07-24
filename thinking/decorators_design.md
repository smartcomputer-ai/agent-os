
# Wit and Query Decorators

Here is how i'd like to use decorators

```python

class TestMessage(BaseModel): #pydantic
    hello:str
    world:str

class WitState(BaseState): #wit
    hellos:List[str] = []

wit = Wit()

@wit.genesis_message
async def on_genesis(msg:InboxMessage):
    print("genesis message", msg.message_id.hex())

@wit.message("test")
async def test_message(content:TestMessage, state: WitState):
    print("test message", content)
    state.hellos.append(content.hello + " " + content.world)

@wit.message("ping")
def test_message(msg:InboxMessage, outbox:Outbox):
    print("test message", msg)
    outbox.send_reply(msg, "pong")

#or
@wit.messages
def test_message(inbox:Inbox):
    print("test message", msg)

#or
@wit.run
async run_wit(inbox:Inbox, outbox:Outbox, core:Core):
    pass


```