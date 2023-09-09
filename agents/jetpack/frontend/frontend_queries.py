
from jinja2 import Environment, TemplateNotFound, select_autoescape
from grit import *
from wit import *
from jetpack.frontend.frontend_wit import FrontendState

app = Wit()

@app.query("web")
async def on_query_web(ctx:QueryContext, state:FrontendState):
    if 'chat' in ctx.query_args_json:
        current_chat = ctx.query_args_json['chat']
        if isinstance(current_chat, list):
            current_chat = current_chat[0]
    else:
        current_chat = 'main'
    template_kwargs = {
        'agent_id': ctx.agent_id.hex(),
        'frontend_id': ctx.actor_id.hex(),
        'chat_actors': {k:v.hex() for k,v in state.chat_actors.items()},
        'chat_titles': state.chat_titles,
        'current_chat': current_chat,
        'current_chat_id': state.chat_actors[current_chat].hex(),
        }
    return await render_template(ctx.core, "/templates/index.html", **template_kwargs)

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