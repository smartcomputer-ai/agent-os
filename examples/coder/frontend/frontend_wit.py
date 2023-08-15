
from grit import *
from wit import *
from common import ChatMessage
from completions import chat_completion

app = Wit()

@app.message("web")
async def on_message_web(content:str, ctx:MessageContext) -> None:
    print("FrontendWit: received message from user")
    # save in the core
    msgt = await ctx.core.gett("messages")
    chat_msg = ChatMessage.from_user(content)
    msgt.makeb(str(chat_msg.id), True).set_as_json(chat_msg)
    # notify the web frontend that the message was received (will be delivered via SSE)
    ctx.outbox.add_reply_msg(ctx.message, str(chat_msg.id), mt="receipt")
    # send new to itself to create a completion
    ctx.outbox.add_new_msg(ctx.actor_id, "completion", mt="completion")

@app.message("completion")
async def on_message_history(msg:InboxMessage, ctx:MessageContext) -> None:
    print("FrontendWit: will create chatbot completion")
    # load message history
    messages_tree = await ctx.core.gett("messages")
    messages = await ChatMessage.load_from_tree(messages_tree)
    print(f"FrontendWit: history has {len(messages)} messages.")
    if len(messages) == 0 :
        return
    # ensure that the last message was from the user
    last_message = messages[-1]
    if last_message.from_name != 'user':
        print(f"FrontendWit: last message was not from 'user', but was from '{last_message.from_name}', will skip.")
        return
    # call OpenAI API
    new_chat_message = await chat_completion(messages, actor_id=ctx.actor_id)
    # save message in history
    messages_tree.makeb(str(new_chat_message.id), True).set_as_json(new_chat_message)
    # notify the web frontend that the message was generated (will be delivered via SSE)
    print("FrontendWit: send receipt to web frontend")
    ctx.outbox.add_new_msg(ctx.agent_id, str(new_chat_message.id), mt="receipt")

