import jsonschema
from .wit_api import *
from .wit_routers import *

# A prototype is an actor that acts as a factory for a certain type of actors
#
# Technically, it can different types of actors, but the prototype notion implies that it's just one type
#
# The prototype is in charge of sending the genesis message to the to-be-created actor
# and also updating all the actors it has created when the the prototype itself is updated
# 
# A prototype core should have 
# - wit:           the prototype wit (the code in this file)
# - prototype:     the core of the actor to be created
#   - wit:         the wit of the target actor
#   - create:      (optional) the arguments to create this actor
#     - schema:    json schema for the create message and args
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
    wit_ref = "external:wit.prototype:wit"
    wrapper_core.makeb('wit').set_as_str(wit_ref)
    wrapper_core.makeb('wit_update').set_as_str(wit_ref)
    wrapper_core.add("prototype", core)
    return wrapper_core

#===================================================================================================
# Create an actor from a prototype
#===================================================================================================
async def create_actor_from_prototype() -> Core:
    pass

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
    p = await _ensure_prototype(ctx.core)
    await _ensure_schema(p, msg)

    new_core = await Core.from_core_id(ctx.store, p.get_as_object_id())

    new_core_create = await new_core.gett("create")
    # if args exist remove them
    if "args" in new_core_create:
        del new_core_create["args"]
    # set the args to the content_id of the create message which should contain only the args
    new_args_id = msg.content_id
    new_core_create.add("args", new_args_id)

    # check in 'created' to see if this actor has already been created with those args
    # we cannot check for the actor id itself because the prototype core might have been updated in the meantime
    # which would change the actor id
    # if the arguments have also been changed as part of the update then a new actor will be created
    new_args_id_str = new_args_id.hex()
    created = await ctx.core.gett("created")
    if new_args_id_str not in created:
        # send the genesis message
        gen_msg = await OutboxMessage.from_genesis(ctx.store, new_core)
        ctx.outbox.add(gen_msg)
        # register the new actor in 'created'
        actor_id = gen_msg.recipient_id
        actor_id_str = actor_id.hex()
        created.makeb(new_args_id_str).set_as_str(actor_id_str)
    else:
        actor_id_str = (await created.getb(new_args_id_str)).get_as_str()
    
    # reply with the new or existing actor_id
    ctx.outbox.add(OutboxMessage.from_reply(msg, actor_id_str, mt="created"))


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
        for key in created:
            actor_id_str = (await created.getb(key)).get_as_str()
            print("sending update to actor: ", actor_id_str)
            actor_id = to_object_id(actor_id_str)
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

async def _ensure_schema(prototype:TreeObject, msg:InboxMessage):
    p_create = await prototype.get("create")
    if p_create is not None:
        #if the prototype has a constructor schema, ensure the create message matches it
        schema = await p_create.get("schema")
        if schema is not None:
            schema_dict = schema.get_as_json()
            create_args:BlobObject = await msg.get_content()
            if not isinstance(create_args, BlobObject):
                raise Exception("Create message must be a blob since a schema is specified: "+schema.get_as_str())
            create_args_dict = create_args.get_as_json()
            try:
                jsonschema.validate(instance=create_args_dict, schema=schema_dict)
            except jsonschema.ValidationError as err:
                raise Exception("Create message does not match schema: "+str(err)) from err

def _is_empty_object_schema(schema):
    if not isinstance(schema, dict):
        return False
    if schema.get("type") != "object":
        return False
    if schema.get("properties", {}) != {}:
        return False
    if schema.get("required", []) != []:
        return False
    if schema.get("additionalProperties", False):
        return False
    return True