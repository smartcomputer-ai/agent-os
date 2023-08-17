import types
from grit import *
from transitions import Machine
from wit import *
from runtime import CoreResolver
from gen_completions import *
from common import *

class CodeGenState(WitState):
    states = [
        'new', 
        'failed', 
        'specified',
        'planning',
        'coding',
        'testing',
        'deployed',
        'completed',
        ]
    
    state:str|None = None
    machine:Machine|None = None
    code_spec:SpecifyCode|None = None
    code_plan:str|None = None
    code_tries:int = 0
    code_errors:str|None = None
    notify:list[ActorId] = []

    def __init__(self):
        super().__init__()

    @property
    def is_job(self):
        return False

    def _after_load(self):
        if(self.machine is None):
            self.machine = Machine(model=self, states=CodeGenState.states, initial='new')
            self.machine.add_transition(trigger='plan', source='specified', dest='planning')
            self.machine.add_transition(trigger='code', source=['planning', 'testing'], dest='coding')
            self.machine.add_transition(trigger='test', source='coding', dest='testing')
            self.machine.add_transition(trigger='deploy', source='testing', dest='deployed')
            self.machine.add_transition(trigger='execute', source='deployed', dest='deployed', unless="is_job")
            self.machine.add_transition(trigger='execute', source='deployed', dest='completed', conditions="is_job")
            self.machine.add_transition(trigger='spec', source='*', dest='specified')
            self.machine.add_transition(trigger='fail', source='*', dest='failed')
        else:
            self.machine.add_model(self, initial=self.state)
    
    def _include_attribute(self, attr_key:str):
        return attr_key.startswith('code_') or attr_key in ['state', 'states', 'machine', 'notify']

def notify_all(state:CodeGenState, outbox:Outbox, msg:any, mt:str|None=None):
    for actor_id in state.notify:
        outbox.add_new_msg(actor_id, msg, mt=mt)


app = Wit()

@app.genesis_message
async def on_genesis_message(msg:InboxMessage, state:CodeGenState):
    print("on_genesis_message")
    print("state:", state.state)

@app.message("spec")
async def on_spec_message(spec:SpecifyCode, msg:InboxMessage, state:CodeGenState, outbox:Outbox, actor_id:ActorId):
    print("on_spec_message")
    state.spec()
    state.code_spec = spec
    if(state.code_spec.max_code_tries is None):
        state.code_spec.max_code_tries = 3
    state.code_tries = 0
    state.code_errors = None
    state.notify.append(msg.sender_id)
    outbox.add_new_msg(actor_id, "plan", mt="plan")

@app.message("plan")
async def on_plan_message(msg:InboxMessage, state:CodeGenState, outbox:Outbox, actor_id:ActorId):
    print("on_plan_message")
    state.plan()
    #todo: convert the spec into a plan using a model completion
    state.code_plan = state.code_spec.task_description
    outbox.add_new_msg(actor_id, "code", mt="code")
    notify_all(state, outbox, CodePlanned(task_description=state.code_spec.task_description, code_plan=state.code_plan), mt="code_planned")

@app.message("code")
async def on_code_message(msg:InboxMessage, state:CodeGenState, core:Core, outbox:Outbox, actor_id:ActorId):
    print("on_code_message")
    state.code()
    state.code_tries += 1
    # copy the code node, if it exists, into the code_test node so that all modules are available
    code_node = await core.get("code")
    if code_node is not None and 'code_test' not in core:
        # setting a new tree node with an existing tree node id is all that is required to copy
        core.add("code_test", code_node.get_as_object_id())
    test_node = await core.gett("code_test") # will create it if it does not exist
    #get the previously generated code (if there is some)
    previous_code = None
    previous_code_blob = await test_node.get("generated.py")
    if previous_code_blob is not None:
        previous_code = previous_code_blob.get_as_str()
    #generate the code
    code = await code_completion(
        state.code_spec.task_description, 
        None, 
        state.code_spec.arguments_spec, 
        state.code_spec.return_spec, 
        previous_code, 
        state.code_errors)
    code = strip_code(code)
    print("generated code (stripped):")
    print("=========================================")
    print(code)
    print("=========================================")
    #save the code
    test_node.makeb("generated.py").set_as_str(code)
    #add an entry point for the resolver
    core.makeb("wit_code_test").set_as_str("/code_test:generated:entry")
    #move state machine forward
    outbox.add_new_msg(actor_id, "test", mt="test")

@app.message("test")
async def on_test_message(msg:InboxMessage, state:CodeGenState, core:Core, outbox:Outbox, actor_id:ActorId, store:ObjectStore):
    print("on_test_message")
    state.test()

    #load the code
    try:
        #use the resolver to load the code and any modules it might be referencing
        resolver = CoreResolver(store)
        entry_func = await resolver.resolve(core.get_as_object_id(), "wit_code_test", is_required=True)
        print("resolved entry func", entry_func)
    except Exception as e:
        print("syntax error, trying to resolve the function:", str(e))
        if state.code_tries >= state.code_spec.max_code_tries:
            outbox.add_new_msg(actor_id, "fail", mt="fail")
        else:
            state.code_errors = str(e)
            outbox.add_new_msg(actor_id, "code", mt="code")
        return
    
    if state.code_spec.test_descriptions is not None:
        print("running tests")
        test_errors = []
        for test_description in state.code_spec.test_descriptions:
            #generate the test data
            print("test description:", test_description)
            test_input = await function_completion(
                "entry", 
                state.code_spec.task_description, 
                state.code_spec.arguments_spec, 
                test_description)
            print("test input:", test_input)
            store_wrapper = StoreWrapper(store)
            function_kwargs = {}
            function_kwargs['input'] = test_input
            function_kwargs['store'] = store_wrapper
            try:
                output = await entry_func(**function_kwargs)
                print("test output:", output)
            except Exception as e:
                print("error trying to execute the generated function:", e)
                if str(e) not in test_errors:
                    test_errors.append(str(e))
        if len(test_errors) > 0:
            print("test errors:", test_errors)
            if state.code_tries >= state.code_spec.max_code_tries:
                outbox.add_new_msg(actor_id, "fail", mt="fail")
            else:
                state.code_errors = "\n".join(test_errors)
                outbox.add_new_msg(actor_id, "code", mt="code")
            return
        
    #the testing succeeded, move forward
    outbox.add_new_msg(actor_id, "deploy", mt="deploy")

@app.message("deploy")
async def on_deploy_message(msg:InboxMessage, state:CodeGenState, core:Core, outbox:Outbox, actor_id:ActorId):
    print("on_deploy_message")
    state.deploy()
    #copy the tested code into the main code node, making it deployed
    code:BlobObject = await core.get_path("code_test/generated.py")
    code_node = await core.gett("code")
    code_node.makeb("generated.py").set_from_blob(code)
    #add an entry point for the resolver
    core.makeb("wit_code").set_as_str("/code:generated:entry")
    notify_all(state, outbox, CodeDeployed(code=code.get_as_str()), mt="code_deployed")


@app.message("execute")
async def on_execute_message(exec:ExecuteCode, state:CodeGenState, core:Core, outbox:Outbox, actor_id:ActorId, store:ObjectStore):
    print("on_execute_message")
    state.execute()
    #use the resolver to load the code and any modules it might be referencing
    resolver = CoreResolver(store)
    try:
        entry_func = await resolver.resolve(core.get_as_object_id(), "wit_code", is_required=True)
    except Exception as e:
        print("error trying to resolve the deployed function:", e)
    
    store_wrapper = StoreWrapper(store)
    function_kwargs = {}
    function_kwargs['input'] = exec.input_arguments
    function_kwargs['store'] = store_wrapper
    try:
        output = await entry_func(**function_kwargs)
        print("test output:", output)
        notify_all(state, outbox, CodeExecuted(input_arguments=exec.input_arguments, output=output), mt="code_executed")
    except Exception as e:
        print("error trying to execute the generated function:", e)

