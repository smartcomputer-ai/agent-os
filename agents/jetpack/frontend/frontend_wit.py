import logging
import re
from grit import *
from wit import *
from jetpack.messages import *
from jetpack.chat.chat_wit import create_chat_actor

logger = logging.getLogger(__name__)

class FrontendState(WitState):
    chat_actors:dict[str, ActorId] = {}
    chat_titles:dict[str, str] = {}

app = Wit()

@app.genesis_message
async def on_genesis(msg:InboxMessage, ctx:MessageContext, state:FrontendState) -> None:
    logger.info("received genesis")
    #create the fist chat
    await create_chat("Main", ctx, state)

@app.message('create-chat')
async def on_create_chat(chat:dict, ctx:MessageContext, state:FrontendState) -> None:
    logger.info("received create chat")
    title = chat['name']
    slug = await create_chat(title, ctx, state)
    ctx.outbox.add_reply_msg(ctx.message, slug, mt="new-chat")

async def create_chat(title:str, ctx:MessageContext, state:FrontendState):
    #create the chat
    slug = slugify(title)
    chat_id = await create_chat_actor(ctx, name=slug)
    state.chat_actors[slug] = chat_id
    state.chat_titles[slug] = title
    return slug

def slugify(text:str):
    text = text.lower()
    return re.sub(r'[\W_]+', '-', text)