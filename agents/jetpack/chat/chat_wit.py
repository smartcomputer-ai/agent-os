import logging
from grit import *
from wit import *
from jetpack.messages import *
from jetpack.coder.coder_wit import create_coder_actor
from jetpack.coder.retriever_wit import create_retriever_actor
from jetpack.chat.chat_completions import chat_completion

logger = logging.getLogger(__name__)

# A "chat" limits a conversation to a single topic and goal. Each chat corresponds to a single chat window in Jetpack.

class ChatState(WitState):
    name:str="Main"

    retriever:ActorId|None = None
    coder:ActorId|None = None

    code_request:CodeRequest|None = None
    code_plan:CodePlanned|None = None
    code_spec:CodeSpec|None = None
    code_deploy:CodeDeployed|None = None
    code_fail:CodeFailed|None = None
    current_execution:CodeExecution|None = None
    last_result:CodeExecuted|None = None

async def create_chat_actor(
        store:ObjectStore, 
        name:str="Main", #allows the differentiation of multiple scopes
        templates:TreeId=None,
        wit_ref:str|None=None,
        wit_query_ref:str|None=None,
        ) -> OutboxMessage:
    #TODO: how to know if this should be external or loaded from a core?
    if wit_ref is not None:
        core = Core.from_external_wit_ref(store, wit_ref=wit_ref)
    else:
        core = Core.from_external_wit_ref(store, "chat_wit:app")
    if wit_query_ref is not None:
        core.makeb("wit_query").set_as_str("external:"+wit_query_ref)
    else:
        core.makeb("wit_query").set_as_str("external:chat_queries:app")

    args = core.maket('args')
    if name is not None:
        args.makeb('name').set_as_str(name)
    #add the templates to the core
    if templates is not None:
        core.add("templates", templates)

    genesis_msg = await OutboxMessage.from_genesis(store, core)
    return genesis_msg


app = Wit()


@app.genesis_message
async def on_genesis(msg:InboxMessage, ctx:MessageContext, state:ChatState) -> None:
    logger.info("received genesis")
    
    args:TreeObject = await ctx.core.get('args')
    if args is not None:
        logger.info("loading args")
        if 'name' in args:
            state.name = (await args.getb('name')).get_as_str()
            logger.info(f"'{state.name}': new chat actor created")

    if state.name is None:
        state.name = "Main"

    #create the downstream actors
    coder_msg = await create_coder_actor(ctx.store, name=f"{state.name} Coder")
    state.coder = coder_msg.recipient_id
    ctx.outbox.add(coder_msg)
    logger.info(f"'{state.name}': created coder actor %s", coder_msg.recipient_id.hex())

    retriever_msg = await create_retriever_actor(ctx.store, forward_to=state.coder)
    state.retriever = retriever_msg.recipient_id
    ctx.outbox.add(retriever_msg)
    logger.info(f"'{state.name}': created retriever actor %s", retriever_msg.recipient_id.hex())


@app.message("web")
async def on_message_web(content:str, ctx:MessageContext, state:ChatState) -> None:
    logger.info(f"'{state.name}': received message from user")
    # save in the core
    msgt = await ctx.core.gett("messages")
    chat_msg = ChatMessage.from_user(content)
    msgt.makeb(str(chat_msg.id), True).set_as_json(chat_msg)
    # notify the web frontend that the message was received (will be delivered via SSE)
    ctx.outbox.add_reply_msg(ctx.message, str(chat_msg.id), mt="receipt")

    #if the last message was from a user, then kick off the models
    if chat_msg.from_name == 'user':
        ctx.outbox.add_new_msg(ctx.actor_id, "complete", mt="complete")
        ctx.outbox.add_reply_msg(ctx.message, "Thinking", mt="thinking")
    else:
        logger.info(f"'{state.name}': last web message was not from 'user', but was from '{chat_msg.from_name}', will skip.")

#========================================================================================
# Frontend States
#========================================================================================
@app.message("complete")
async def on_complete_message(msg:InboxMessage, ctx:MessageContext, state:ChatState) -> None:
    logger.info(f"'{state.name}': completion")
    # load message history
    messages_tree = await ctx.core.gett("messages")
    messages = await ChatMessage.load_from_tree(messages_tree)
    if len(messages) == 0 :
        return
    # ensure that the last message was from the user
    last_message = messages[-1]
    if last_message.from_name != 'user':
        logger.info(f"'{state.name}': last message was not from 'user', but was from '{last_message.from_name}', will skip.")
        return
    
    # chat completion, and process result
    result = await chat_completion(messages, state.code_spec)
    
    if isinstance(result, str):
        logger.info(f"'{state.name}': completion is normal chat message.")
        chat_message = ChatMessage.from_actor(result, ctx.actor_id)

    elif isinstance(result, CodeRequest):
        logger.info(f"'{state.name}': completion is a CodeRequest.")
        # message the retriever actor to generate the code
        if state.retriever is not None:
            ctx.outbox.add_new_msg(state.retriever, result, mt="request")
            logger.info(f"'{state.name}': sent CodeRequest mesage to retriever: {state.retriever.hex()}")
        else:
            logger.info(f"'{state.name}': retriever peer not found")
            return
        
        if state.code_request is None:
            msg = "I will generate the requested functionality:"
        else:
            msg = "I will generate the requested changes:"
        msg += f"\n```\n{result.task_description}\n```\n"
        if(result.input_examples is not None):
            msg += f"Here are some examples for the function input:\n```\n{result.input_examples}\n```\n"

        state.code_request = result
        # create a chat message for the frontend
        chat_message = ChatMessage.from_actor(msg, ctx.actor_id) 
        ctx.outbox.add_new_msg(ctx.agent_id, "Start Coding", mt="thinking")

    
    elif isinstance(result, CodeExecution):
        logger.info(f"'{state.name}': completion is a CodeExecution.")
        if state.code_spec is None:
            logger.info(f"'{state.name}': code_spec is None, will skip.")
            return
        # message the coder actor to execute the code
        if state.coder is not None:
            ctx.outbox.add_new_msg(state.coder, result, mt="execute")
            logger.info(f"'{state.name}': sent CodeExecution mesage to coder: {state.coder.hex()}")
        else:
            logger.info(f"'{state.name}': coder peer not found")
            return
        
        msg = "I will call the function with the following arguments:"
        if(result.input_arguments is not None):
            msg += f"\n```\n{result.input_arguments}\n```\n"

        state.current_execution = result
        # create a chat message for the frontend
        chat_message = ChatMessage.from_actor(msg, ctx.actor_id)
        ctx.outbox.add_new_msg(ctx.agent_id, "Executing", mt="thinking")

    # save message in history
    messages_tree.makeb(str(chat_message.id), True).set_as_json(chat_message)
    # notify the web frontend that the message was generated (will be delivered via SSE)
    logger.info(f"'{state.name}': send receipt to web frontend")
    ctx.outbox.add_new_msg(ctx.agent_id, str(chat_message.id), mt="receipt")


#========================================================================================
# Coder Callbacks
#========================================================================================

@app.message("code_speced")
async def on_message_code_speced(spec:CodeSpec, ctx:MessageContext, state:ChatState) -> None:
    logger.info(f"'{state.name}': received callback: code_speced")
    state.code_spec = spec
    ctx.outbox.add_new_msg(ctx.agent_id, "artifact", mt="artifact")

@app.message("code_planned")
async def on_message_code_planned(plan:CodePlanned, ctx:MessageContext, state:ChatState) -> None:
    logger.info(f"'{state.name}': received callback: code_planned")
    state.code_plan = plan
    if state.code_plan.plan is not None:
        #todo: also output as chat message
        ctx.outbox.add_new_msg(ctx.agent_id, "artifact", mt="artifact")

@app.message("code_deployed")
async def on_message_code_deployed(code:CodeDeployed, ctx:MessageContext, state:ChatState) -> None:
    logger.info(f"'{state.name}': received callback: code_deployed")
    state.code_deploy = code
    if state.code_deploy.code is not None:
        # render the code and notify frontend
        chat_message = ChatMessage.from_actor(f"Here is the function and code I generated:\n```\n{code.code}\n```\n", ctx.actor_id)
        messages_tree = await ctx.core.gett("messages")
        messages_tree.makeb(str(chat_message.id), True).set_as_json(chat_message)
        ctx.outbox.add_new_msg(ctx.agent_id, str(chat_message.id), mt="receipt")
        ctx.outbox.add_new_msg(ctx.agent_id, "artifact", mt="artifact")

@app.message("code_executed")
async def on_message_code_executed(exec:CodeExecuted, ctx:MessageContext, state:ChatState) -> None:
    logger.info(f"'{state.name}': received callback: code_executed")
    state.current_execution = None
    state.last_result = exec
    content = ""
    links = []
    for key in list(exec.output):
        if isinstance(exec.output[key], str) and is_object_id_str(exec.output[key]):
            obj_id_str = exec.output[key]
            obj_id = to_object_id(obj_id_str)
            #figure out what's in the object
            obj = await ctx.store.load(obj_id)
            if not is_blob(obj):
                continue
            blob = BlobObject(obj, obj_id)

            #see if the content type is set
            content_type = blob.get_header("Content-Type")
            if content_type is None and blob.get_header("ct") == "s":
                content_type = "text/plain"
            if content_type is None and blob.get_header("ct") == "j":
                content_type = "application/json"
            
            if content_type is None:
                continue

            #if the content type is an image, then display it as an image
            if content_type.startswith("image/"):
                image_url = f"../../../../grit/objects/{obj_id_str}"
                content += f"Here is the image I generated:\n![]({image_url})\n"
            #if the content type is text, then display it as text
            elif content_type.startswith("text/"):
                content += f"Here is what I generated:\n\n{blob.get_as_str()}\n"
            #if the content type is json, then display it as json
            elif content_type.startswith("application/json"):
                content += f"Here is the JSON I generated:\n```\n{json.dumps(blob.get_as_json(), indent=2)}\n```\n"
            
            links.append(f"[{key}: {obj_id_str}](../../../../grit/objects/{obj_id_str})")

            #pop from the dictionary
            exec.output.pop(key)

    if len(links) > 0:
        content += "Here are the links to the generated objects:\n"
        for link in links:
            content += f"- {link}\n"
            
    if len(exec.output) > 0:
        content += f"Here is the result from the execution:\n```\n{exec.output}\n```"

    chat_message = ChatMessage.from_actor(content, ctx.actor_id)
    messages_tree = await ctx.core.gett("messages")
    messages_tree.makeb(str(chat_message.id), True).set_as_json(chat_message)
    ctx.outbox.add_new_msg(ctx.agent_id, str(chat_message.id), mt="receipt")
    ctx.outbox.add_new_msg(ctx.agent_id, "artifact", mt="artifact")

@app.message("code_failed")
async def on_message_code_failed(fail:CodeFailed, ctx:MessageContext, state:ChatState) -> None:
    logger.warn(f"'{state.name}': received callback: code_failed")
    state.code_fail = fail
    if state.code_fail.errors is not None:
        #todo: also output as chat message
        ctx.outbox.add_new_msg(ctx.agent_id, "artifact", mt="artifact")