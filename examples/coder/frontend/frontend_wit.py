
from grit import *
from wit import *
from common import *

class FrontendState(WitState):
    peers:dict[str, ActorId] = {}

app = Wit()

@app.message("notify_genesis")
async def on_message_notify(content:dict, state:FrontendState, agent_id:ActorId) -> None:
    print("FrontendWit: received notify_genesis")
    state.peers[content['actor_name']] = to_object_id(content['actor_id'])
    state.peers['agent'] = agent_id

@app.message("web")
async def on_message_web(content:str, ctx:MessageContext, state:FrontendState) -> None:
    print("FrontendWit: received message from user")
    #save in the core
    msgt = await ctx.core.gett("messages")
    chat_msg = ChatMessage.from_user(content)
    msgt.makeb(str(chat_msg.id), True).set_as_json(chat_msg)
    msgt_id = await msgt.persist(ctx.store)
    ctx.outbox.add(OutboxMessage.from_reply(ctx.message, str(chat_msg.id), mt="receipt"))
    #send new msg history to head
    if('head' in state.peers):
        print('FrontendWit: sending history update to head')
        ctx.outbox.add(OutboxMessage.from_new(state.peers['head'], msgt_id, mt="history"))
    else:
        print('FrontendWit: dont know head')

@app.message("head_reply")
async def on_message_head_reply(content:ChatMessage, ctx:MessageContext, state:FrontendState) -> None:
    print("FrontendWit: received reply message from head")
    #save in the core
    msgt = await ctx.core.gett("messages")
    msgt.makeb(str(content.id), True).set_as_json(content)
    #send reciept
    print("send receipt")
    ctx.outbox.add(OutboxMessage.from_new(state.peers['agent'], str(content.id), mt="receipt"))
