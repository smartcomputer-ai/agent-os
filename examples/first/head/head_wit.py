from grit import *
from wit import *
from common import *

app = Wit()
@app.message("history")
async def on_message_history(messages_tree:TreeObject, ctx:MessageContext) -> None:
    print("HeadWit: received mesage history")
    messages = await ChatMessage.load_from_tree(messages_tree)
    print("HeadWit: there are messages:", len(messages))
    if len(messages) == 0 :
        return
    #ensure that the last message was from the user
    last_message = messages[-1]
    if last_message.from_name != 'user':
        return
    new_chat_message = await chat_completion(messages, actor_id=ctx.actor_id)
    print("HeadWit: sending new chat message to frontend", new_chat_message.content)
    ctx.outbox.add(OutboxMessage.from_reply(
        ctx.message, 
        BlobObject.from_json(new_chat_message), 
        mt="head_reply"))
        



            
