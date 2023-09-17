import logging
from jinja2 import Environment, TemplateNotFound, select_autoescape
from grit import *
from wit import *
from jetpack.messages import ChatMessage
from jetpack.chat.chat_wit import ChatState

logger = logging.getLogger(__name__)

app = Wit()

@app.query("messages")
async def on_query_messages(core:Core, messagekey:str=None):
    message_filter = messagekey
    messages = await ChatMessage.load_from_tree(await core.gett("messages"), message_filter)
    logger.info(f"messages: {len(messages)}, filter: {message_filter}")
    return await render_template(core, "/templates/chat_messages.html", messages=messages)

@app.query("artifacts")
async def on_query_artifacts(core:Core, state:ChatState, ctx:QueryContext):
    url_path = f"../../{ctx.actor_id.hex()}/query"
    artifacts = []
    if state.code_spec is not None:
        artifacts.append({"title": "Specification", "url": f"{url_path}/artifact-spec", "emoji": "📋"})
    if state.code_plan is not None and state.code_plan.plan is not None:
        artifacts.append({"title": "Plan", "url": f"{url_path}/artifact-plan", "emoji": "📃"})
    if state.code_deploy is not None and state.code_deploy.code is not None:
        artifacts.append({"title": "Code", "url": f"{url_path}/artifact-code", "emoji": "▶️"})
    return await render_template(core, "/templates/artifacts.html", artifacts=artifacts)

@app.query("artifact-spec")
async def on_query_artifacts_spec(core:Core, state:ChatState):
    logger.info("on_query_artifacts_spec")
    return state.code_spec

@app.query("artifact-plan")
async def on_query_artifacts_plan(core:Core, state:ChatState):
    logger.info("on_query_artifacts_plan")
    return state.code_plan.plan

@app.query("artifact-code")
async def on_query_artifacts_code(core:Core, state:ChatState):
    logger.info("on_query_artifacts_code")
    return state.code_deploy.code

env = Environment(autoescape=select_autoescape())
async def render_template(core:Core, template_path, **kwargs) -> BlobObject:
    template_blob = await core.get_path(template_path)
    if(template_blob is None):
        raise TemplateNotFound(f"Template not found: {template_path}")
    template_str = template_blob.get_as_str()
    template = env.from_string(template_str)
    rendered = template.render(**kwargs)
    rendered_blob = BlobObject.from_str(rendered)
    rendered_blob.set_headers_empty()
    rendered_blob.set_header('Content-Type', 'text/html')
    return rendered_blob