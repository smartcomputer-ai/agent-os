
from grit import *
from transitions import Machine
from wit import *
from common import *
from frontend_completions import code_spec_completion, code_exec_completion

class FrontendState(WitState):
    peers:dict[str, ActorId] = {}
    states = [
        'specifying', 
        'waiting_for_code', 
        'executing',
        ]
    state:str|None = None
    machine:Machine|None = None
    code_spec:SpecifyCode|None = None

    def _after_load(self):
        if(self.machine is None):
            self.machine = Machine(model=self, states=FrontendState.states, initial='specifying')
            self.machine.add_transition(trigger='send_spec', source='specifying', dest='waiting_for_code')
            self.machine.add_transition(trigger='deployed', source='waiting_for_code', dest='executing')
            self.machine.add_transition(trigger='execute', source='executing', dest='executing')
        else:
            self.machine.add_model(self, initial=self.state)

    def _include_attribute(self, attr_key:str):
        return attr_key in ['code_spec', 'state', 'states', 'machine', 'peers']

app = Wit()

#========================================================================================
# Frontend Main
#========================================================================================
@app.message("notify_genesis")
async def on_message_notify(content:dict, state:FrontendState, agent_id:ActorId) -> None:
    print("FrontendWit: received notify_genesis")
    state.peers[content['actor_name']] = to_object_id(content['actor_id'])
    state.peers['agent'] = agent_id

@app.message("web")
async def on_message_web(content:str, ctx:MessageContext, state:FrontendState) -> None:
    print("FrontendWit: received message from user")
    # save in the core
    msgt = await ctx.core.gett("messages")
    chat_msg = ChatMessage.from_user(content)
    msgt.makeb(str(chat_msg.id), True).set_as_json(chat_msg)
    # notify the web frontend that the message was received (will be delivered via SSE)
    ctx.outbox.add_reply_msg(ctx.message, str(chat_msg.id), mt="receipt")
    # notify itself to generate a response
    if state.state == 'specifying':
        ctx.outbox.add_new_msg(ctx.actor_id, "specify", mt="specify")
    elif state.state == 'waiting_for_code':
        ctx.outbox.add_new_msg(ctx.actor_id, "coding", mt="coding")
    elif state.state == 'executing':
        ctx.outbox.add_new_msg(ctx.actor_id, "execute", mt="execute")

#========================================================================================
# Frontend States
#========================================================================================
@app.message("specify")
async def on_message_specify(msg:InboxMessage, ctx:MessageContext, state:FrontendState) -> None:
    print("FrontendWit: will specify code")
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
    new_chat_message, code_spec = await code_spec_completion(messages, actor_id=ctx.actor_id)
    # save message in history
    messages_tree.makeb(str(new_chat_message.id), True).set_as_json(new_chat_message)
    # notify the web frontend that the message was generated (will be delivered via SSE)
    print("FrontendWit: send receipt to web frontend")
    ctx.outbox.add_new_msg(ctx.agent_id, str(new_chat_message.id), mt="receipt")

    if code_spec is not None:
        print("FrontendWit: code_spec was generated")
        #render the code spec as a frontend message
        code_spec_content = f"{code_spec.task_description}\n\nInputs:\n```\n{code_spec.arguments_spec}\n```\nOutputs:\n```\n{code_spec.return_spec}\n```\nTests:\n```\n{code_spec.test_descriptions}\n```\n"
        code_spec_message = ChatMessage.from_actor(code_spec_content, ctx.actor_id)
        print("FrontendWit: code_spec:", code_spec_content)
        messages_tree.makeb(str(code_spec_message.id), True).set_as_json(code_spec_message)
        ctx.outbox.add_new_msg(ctx.agent_id, str(code_spec_message.id), mt="receipt")

        #move the state machine forward
        state.code_spec = code_spec
        state.send_spec()

        #message the coder actor to generate the code
        if 'coder' in state.peers:
            ctx.outbox.add_new_msg(state.peers['coder'], code_spec, mt="spec")
        else:
            print("FrontendWit: coder peer not found")

@app.message("coding")
async def on_message_coding(msg:InboxMessage, ctx:MessageContext, state:FrontendState) -> None:
    print("FrontendWit: Already coding")
    # load message history
    messages_tree = await ctx.core.gett("messages")
    messages = await ChatMessage.load_from_tree(messages_tree)
    if len(messages) == 0 :
        return
    # ensure that the last message was from the user
    last_message = messages[-1]
    if last_message.from_name != 'user':
        return
    
    new_chat_message = ChatMessage.from_actor("I am already coding... please wait until I'm done.", ctx.actor_id)
    messages_tree.makeb(str(new_chat_message.id), True).set_as_json(new_chat_message)
    print("FrontendWit: send receipt to web frontend")
    ctx.outbox.add_new_msg(ctx.agent_id, str(new_chat_message.id), mt="receipt")

@app.message("execute")
async def on_message_coding(msg:InboxMessage, ctx:MessageContext, state:FrontendState) -> None:
    print("FrontendWit: Execute")
    # load message history
    messages_tree = await ctx.core.gett("messages")
    messages = await ChatMessage.load_from_tree(messages_tree)
    if len(messages) == 0 :
        return
    # ensure that the last message was from the user
    last_message = messages[-1]
    if last_message.from_name != 'user':
        return
    
    new_chat_message, exec_code = await code_exec_completion(messages, state.code_spec, actor_id=ctx.actor_id)
    messages_tree.makeb(str(new_chat_message.id), True).set_as_json(new_chat_message)
    print("FrontendWit: send receipt to web frontend")
    ctx.outbox.add_new_msg(ctx.agent_id, str(new_chat_message.id), mt="receipt")

    if exec_code is not None:
        print("FrontendWit: exec_code was generated")
        #render the code spec as a frontend message
        exec_content = f"Inputs:\n```\n{exec_code.input_arguments}\n```"
        exec_message = ChatMessage.from_actor(exec_content, ctx.actor_id)
        print("FrontendWit: exec_content:", exec_content)
        messages_tree.makeb(str(exec_message.id), True).set_as_json(exec_message)
        ctx.outbox.add_new_msg(ctx.agent_id, str(exec_message.id), mt="receipt")

        #move the state machine forward (which will remain on exec)
        state.execute()

        #message the coder actor to execute the code
        if 'coder' in state.peers:
            ctx.outbox.add_new_msg(state.peers['coder'], exec_code, mt="execute")
        else:
            print("FrontendWit: coder peer not found")
    else:
        print("FrontendWit: no function call")

#========================================================================================
# Coder Callbacks
#========================================================================================
@app.message("code_deployed")
async def on_message_code_deployed(code:CodeDeployed, ctx:MessageContext, state:FrontendState) -> None:
    print("FrontendWit: received code_deployed")
    state.deployed()
    
    # render the code and notify frontend
    code_message = ChatMessage.from_actor(f"Here is the code I generated:\n```\n{code.code}\n```\n", ctx.actor_id)
    messages_tree = await ctx.core.gett("messages")
    messages_tree.makeb(str(code_message.id), True).set_as_json(code_message)
    ctx.outbox.add_new_msg(ctx.agent_id, str(code_message.id), mt="receipt")

@app.message("code_executed")
async def on_message_code_executed(exec:CodeExecuted, ctx:MessageContext, state:FrontendState) -> None:
    print("FrontendWit: received code_executed")
    state.execute()
    
    # render the code and notify frontend
    code_message = ChatMessage.from_actor(f"Here is the result from the execution:\n```\n{exec.output}\n```", ctx.actor_id)
    messages_tree = await ctx.core.gett("messages")
    messages_tree.makeb(str(code_message.id), True).set_as_json(code_message)
    ctx.outbox.add_new_msg(ctx.agent_id, str(code_message.id), mt="receipt")
