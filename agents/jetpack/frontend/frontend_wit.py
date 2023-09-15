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
    #find the templates to pass to the new chat
    templ = await ctx.core.get("templates")
    if templ is None:
        raise Exception("templates not found")
    templ_id = templ.get_as_object_id()

    #create the chat
    slug = slugify(title)
    genesis_msg = await create_chat_actor(ctx.store, name=slug, templates=templ_id)
    ctx.outbox.add(genesis_msg)
    state.chat_actors[slug] = genesis_msg.recipient_id
    state.chat_titles[slug] = title
    return slug


def slugify(text):
    text = text.lower()
    return re.sub(r'[\W_]+', '-', text)