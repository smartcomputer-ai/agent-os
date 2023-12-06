from jsonschema import validate
from grit import *
from wit import *

# The idea of an environment, here, is closely related to the env from Lisp or Subjects in Urbit.
# It's basically a recursive structure that is storngly typed. But in the case of Agent OS, it can
# be accessed by different programming languages.
# Other "processes" / wits, update the env they are "attached to".
#
# Structure:
# - content types: a list containing all valid content types for this env
# - schemas: valid json schemas for json structured content
# - vars: the actual instances of the data

#========================================================================================
# Setup & State
#========================================================================================
CONTENT_TYPES = "cts"
SCHEMAS = "schemas"
VARIABLES = "vars"

logger = logging.getLogger(__name__)

async def create_env_actor(
        ctx:MessageContext,
        name:str,
        enforce_content_type:bool = True,
        enforce_json_schema:bool = True
        ) -> ActorId:
    config = EnvConfig()
    config.name = name
    config.enforce_content_type = enforce_content_type
    config.enforce_json_schema = enforce_json_schema
    return await create_actor_from_prototype_with_state(
        ctx.prototype_actors["scope"], 
        config, 
        ctx.request_response, 
        ctx.store)

class EnvConfig(WitState):
    name:str
    enforce_content_type:bool = True
    enforce_json_schema:bool = True

#========================================================================================
# Wit
#========================================================================================
wit = Wit()

@wit.message("set")
async def on_message_set(msg:InboxMessage, ctx:MessageContext, state:EnvConfig):
    name = await _validate_msg(msg, ctx.core, state)
    vars = await ctx.core.gett(VARIABLES)
    if name in vars:
        del vars[name]
    vars.add(name, msg.content_id)

async def _validate_msg(msg:InboxMessage, core:Core, state:EnvConfig) -> str:
    content_type = get_msg_content_type(msg)
    if state.enforce_content_type:
        if content_type is None:
            raise Exception("Message contains no 'ct' or 'Content-Type' header, but it is required.") 
        cts:list[str] = (await core.getb(CONTENT_TYPES)).get_as_json()
        if content_type not in cts:
            raise Exception(f"Message content type '{content_type}' not in env.") 

    if state.enforce_json_schema:
        if content_type is None:
            raise Exception("Cannot enforce schema if content type is not set.") 
        if content_type == 'application/json':
            if "schema" not in msg.headers:
                raise Exception("Message contains no 'schema' header, but is required.")
            schema_name = msg.headers["schema"]
            schemas = await core.gett(SCHEMAS)
            if schema_name not in schemas:
                raise Exception(f"Message schema '{schema_name}' not in env.")
            schema = (await schemas.getb(schema_name)).get_as_json()
            msg_content = await msg.get_content()
            validate(msg_content, schema)

    if "name" not in msg.headers:
        raise Exception("Message contains no variable 'name' header, but it is required.")
    variable_name = msg.headers["name"]
    if variable_name == '':
        raise Exception(f"Message variable 'name' is not valid ('{variable_name}').")
    return variable_name

def get_msg_content_type(msg:InboxMessage):
    if "Content-Type" in msg.headers:
        return msg.headers["Content-Type"]
    elif "ct" in msg.headers:
        ct = msg.headers["ct"]
        if ct == "b":
            return "application/octet-stream"
        if ct == "s":
            return "text/plain"
        if ct == "j":
            return "application/json"
    return None

#========================================================================================
# Queries
#========================================================================================

@wit.query("get")
async def query_get(core:Core, ctx:QueryContext):
    pass
