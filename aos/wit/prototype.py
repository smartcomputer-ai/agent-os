from .wit_api import *
from .wit_routers import *
import uuid

# A prototype is an actor that acts as a factory for a certain type of actor
#
# The prototype is in charge of sending the genesis message to the to-be-created actor
# and also updating all the actors it has created when the the prototype itself is updated
# 
# A prototype core should have 
# - wit:           the prototype wit (the code in this file)
# - prototype:     the core of the actor to be created
#   - wit:         the wit of the target actor
#   - create:      (optional) the arguments to create this actor
#     - args:      (later) will only be created on 'create', and will be the contents of the create message (can be a tree or blob)
#                  but if schema is provided, then it will need to be a json blob that matches the schema
# - created:       a list of all the actors created by this prototype and who created them

#===================================================================================================
# Create a prototype core
# Wraps a given core in a prototype core/wit
#===================================================================================================
def wrap_in_prototype(core:Core) -> Core:
    # create the prototype core
    wrapper_core = Core(None, None, None)
    # add the prototype wit
    wit_ref = "external:aos.wit.prototype:wit"
    wrapper_core.makeb('wit').set_as_str(wit_ref)
    wrapper_core.makeb('wit_genesis').set_as_str(wit_ref)
    wrapper_core.makeb('wit_update').set_as_str(wit_ref)
    wrapper_core.add("prototype", core)
    return wrapper_core

#===================================================================================================
# Create an actor from a prototype
#===================================================================================================
def create_actor_from_prototype_msg(
        prototype_id:ActorId, 
        args:ValidMessageContent|None,
        instance:str|None=None,
        ) -> OutboxMessage:
    if args is None:
        args = {}
    msg = OutboxMessage.from_new(prototype_id, args, is_signal=True, mt="create")
    if instance is not None:
        msg.headers["instance"] = instance
    return msg

async def create_actor_from_prototype_msg_with_state(
        prototype_id:ActorId, 
        state:WitState, 
        store:ObjectStore,
        instance:str|None=None,
        ) -> OutboxMessage:
    state_tree_id = await state._persist_to_tree_id(store)
    create_msg = create_actor_from_prototype_msg(prototype_id, state_tree_id, instance=instance)
    return create_msg

async def create_actor_from_prototype(
        prototype_id:ActorId, 
        args:ValidMessageContent|None, 
        request_response:RequestResponse,
        instance:str|None=None,
        ) -> ActorId:
    create_msg = create_actor_from_prototype_msg(prototype_id, args, instance=instance)
    response = await request_response.run(create_msg, ["created"], 1.0)
    created_actor_id_str = (await response.get_content()).get_as_str()
    return to_object_id(created_actor_id_str)

async def create_actor_from_prototype_with_state(
        prototype_id:ActorId, 
        state:WitState, 
        request_response:RequestResponse, 
        store:ObjectStore,
        instance:str|None=None,
        ) -> ActorId:
    state_tree_id = await state._persist_to_tree_id(store)
    return await create_actor_from_prototype(prototype_id, state_tree_id, request_response, instance=instance)

async def get_prototype_args(core:Core) -> TreeObject|BlobObject|None:
    state = await core.get("state")
    if state is None:
        return None
    return await state.get("args")

async def get_prototype_args_as_json(core:Core) -> dict:
    args = await get_prototype_args(core)
    if args is None:
        return {}
    if not isinstance(args, BlobObject):
        raise Exception(f"Prototype args must be a blob, but got '{type(args)}'.")
    return args.get_as_json()

async def get_prototype_args_as_model(core:Core, pydantic_type:Type[BaseModel]) -> BaseModel:
    args = await get_prototype_args(core)
    if args is None:
        return {}
    if not isinstance(args, BlobObject):
        raise Exception(f"Prototype args must be a blob, but got '{type(args)}'.")
    return args.get_as_model(pydantic_type)

#===================================================================================================
# Prototype wit function implementaion
# Accepts only, genesis, "create" and update messages
#===================================================================================================
wit = Wit()

@wit.genesis_message
async def on_genesis(msg:InboxMessage, ctx:MessageContext):
    # ensure that the core is well formed
    await _ensure_prototype(ctx.core)
    
@wit.message("create")
async def on_create(msg:InboxMessage, ctx:MessageContext):

    # the "instance" identifies the instance that is being created
    # it can be supplied by the caller to make sure the exact instance is being created
    # in fact, it is preferable to supply it, because it will also be returned in the reply
    # which makes it easier to correlate the created actor with the initial request
    if "instance" in msg.headers:
        instance = msg.headers["instance"]
    else:
        instance = str(uuid.uuid4())

    p = await _ensure_prototype(ctx.core)
    new_core = await Core.from_core_id(ctx.store, p.get_as_object_id())

    # either the create message is a tree, in which case we assume is a WitState compatible tree,
    # or it is a blob, in which case we just put it in state under args
    msg_contents = await msg.get_content()
    if isinstance(msg_contents, TreeObject):
        await new_core.merge(msg_contents)
    else:
        if "args" in new_core:
            del new_core["args"]
        new_core.add("args", msg.content_id)

    # Check in 'created' to see if this actor has already been created with those args/state
    # we cannot check for the hypothetical future actor id itself because the prototype core might have been updated in the meantime,
    # which would change the actor id. (so that's why we have to maintain a seperate "created" list)
    # For example, if the state changes as part as part of the update then a new actor will be created.
    created = await ctx.core.gett("created")
    if instance not in created:
        # send the genesis message
        gen_msg = await OutboxMessage.from_genesis(ctx.store, new_core)
        ctx.outbox.add(gen_msg)
        # register the new actor in 'created'
        actor_id = gen_msg.recipient_id
        created.add(instance, actor_id)
    else:
        actor_id = created.get_id(instance)
    
    # reply with the new or existing actor_id
    reply = OutboxMessage.from_reply(msg, actor_id, mt="created")
    reply.headers["instance"] = instance
    ctx.outbox.add(reply)

@wit.update_message
async def on_update(msg:InboxMessage, ctx:MessageContext):
    #get the current prototype core and the new one and merge them
    p_current = await _ensure_prototype(ctx.core)
    p_new = await _ensure_prototype(await msg.get_content(), check_wit=False)
    p_new_id = p_new.get_as_object_id()

    core_current = await Core.from_core_id(ctx.store, p_current.get_as_object_id())
    core_new = await Core.from_core_id(ctx.store, p_new_id)

    await core_current.merge(core_new)

    # update all actors that have been created based on this prototype
    created:TreeObject = await ctx.core.get("created")
    if created is not None:
        for instance in created:
            actor_id = created.get_id(instance)
            print("sending update to actor: ", actor_id.hex())
            # do not send the whole merged core, because the update might be just a few objects not an entire core
            # and so it should be forwarded accordingly
            ctx.outbox.add(OutboxMessage.from_update(actor_id, p_new_id))

#===================================================================================================
# Utils
#===================================================================================================
async def _ensure_prototype(core:Core, check_wit:bool=True):
    # ensure that the core is well formed
    p:TreeObject = await core.get("prototype")
    if p is None:
        raise Exception("No 'prototype' specified in the core")
    if check_wit:
        p_wit:BlobObject = await p.get("wit")
        if p_wit is None:
            raise Exception("No 'wit' specified in the prototype")
    return p