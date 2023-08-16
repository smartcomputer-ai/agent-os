
from grit import *
from transitions import Machine
from wit import *
from common import *
from frontend_completions import code_spec_completion

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
    if state.state == 'specifying':
        # send new to itself to create a completion
        ctx.outbox.add_new_msg(ctx.actor_id, "specify", mt="specify")
    #elif state.state == 'waiting_for_code':


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
        code_spec_content = f"{code_spec.task_description}\nInputs:\n```{code_spec.arguments_spec}```\nOutputs:\n```{code_spec.return_spec}```\nTests:\n```{code_spec.test_descriptions}```"
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


