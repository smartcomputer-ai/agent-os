import logging
from grit import *
from wit import *
from wit.wit_routers import _Wrapper
from runtime import CoreResolver
from tools import StoreWrapper
from jetpack.messages import *
from jetpack.coder.coder_completions import *

logger = logging.getLogger(__name__)

class CoderState(WitState):
    name:str = "coder"
    notify:set[ActorId] = set()

    code_spec:CodeSpec|None = None
    code_plan:str|None = None
    code_tries_max:int = 3
    code_tries:int = 0
    code_errors:str|None = None
    job_execution:CodeExecution|None = None

def reset_code(state:CoderState):
    state.code_plan = None
    state.code_tries = 0
    state.code_tries_max = 3
    state.code_errors = None


def notify_all(state:CoderState, outbox:Outbox, msg:any, mt:str|None=None):
    for actor_id in state.notify:
        outbox.add_new_msg(actor_id, msg, mt=mt)

async def create_coder_actor(
        store:ObjectStore, 
        name:str="coder", #allows the differentiation of multiple coders
        spec:CodeSpec|None=None,
        job_execution:CodeExecution|None=None,
        wit_ref:str|None=None,
        ) -> OutboxMessage:
    #TODO: how to know if this should be external or loaded from a core?
    if wit_ref is not None:
        core = Core.from_external_wit_ref(store, wit_ref=wit_ref)
    else:
        core = Core.from_external_wit_ref(store, "coder_wit:app")

    args = core.maket('args')
    if name is not None:
        args.makeb('name').set_as_str(name)
    if spec is not None:
        args.makeb('code_spec').set_as_json(spec)
    if job_execution is not None:
        args.makeb('job_execution').set_as_json(job_execution)
    
    genesis_msg = await OutboxMessage.from_genesis(store, core)
    return genesis_msg


app = Wit()

@app.genesis_message
async def on_genesis_message(msg:InboxMessage, core:Core, state:CoderState, outbox:Outbox, actor_id:ActorId):
    logger.info("on_genesis_message")
    #copy args into state
    args:TreeObject = await core.get('args')
    if args is not None:
        logger.info("loading args")
        if 'name' in args:
            state.name = (await args.getb('name')).get_as_str()
        if 'code_spec' in args:
            state.code_spec = (await args.getb('code_spec')).get_as_model(CodeSpec)
        if 'job_execution' in args:
            state.job_execution = (await args.getb('job_execution')).get_as_model(CodeExecution)
        # add the sender to the notify list
        # only if args are provided, because we can assume the sender is another actor
        state.notify.add(msg.sender_id)
    logger.info("is job: %s", state.job_execution is not None)
    # if code specs were provided, move right to the spec state
    # and send message to move on to the plan state 
    if state.code_spec is not None:
        reset_code(state)
        outbox.add_new_msg(actor_id, "plan", mt="plan")

@app.message("spec")
async def on_spec_message(spec:CodeSpec, msg:InboxMessage, state:CoderState, outbox:Outbox, actor_id:ActorId):
    logger.info("on_spec_message")
    state.code_spec = spec
    reset_code(state)
    state.notify.add(msg.sender_id)
    outbox.add_new_msg(actor_id, "plan", mt="plan")
    notify_all(state, outbox, spec, mt="code_speced")


@app.message("plan")
async def on_plan_message(msg:InboxMessage, state:CoderState, ctx:MessageContext):
    logger.info("on_plan_message")
    if state.code_spec.input_spec is None or state.code_spec.output_spec is None:
        logger.info("generating input and/or output specs")
        input_spec, output_spec = await inputoutput_completion(
            state.code_spec.task_description,
            state.code_spec.input_examples,
            state.code_spec.input_spec,
            state.code_spec.output_spec,
            )
        if input_spec is not None:
            logger.info("generated input_spec: %s", input_spec)
            state.code_spec.input_spec = input_spec
        if output_spec is not None:
            logger.info("generated output_spec: %s", output_spec)
            state.code_spec.output_spec = output_spec

    #todo: convert the spec into a plan using a model completion
    state.code_plan = state.code_spec.task_description
    ctx.outbox.add_new_msg(ctx.actor_id, "code", mt="code")
    ctx.outbox.add_new_msg(ctx.agent_id, f"Coding: {state.name}", mt="thinking")
    notify_all(state, ctx.outbox, state.code_spec, mt="code_speced")
    notify_all(state, ctx.outbox, CodePlanned(plan=state.code_plan), mt="code_planned")


@app.message("code")
async def on_code_message(msg:InboxMessage, state:CoderState, ctx:MessageContext):
    logger.info("on_code_message")
    state.code_tries += 1
    # copy the code node, if it exists, into the code_test node so that all modules are available
    code_node = await ctx.core.get("code")
    if code_node is not None and 'code_test' not in ctx.core:
        # setting a new tree node with an existing tree node id is all that is required to copy
        ctx.core.add("code_test", code_node.get_as_object_id())
    test_node = await ctx.core.gett("code_test") # will create it if it does not exist
    #get the previously generated code (if there is some)
    previous_code = None
    previous_code_blob = await test_node.get("generated.py")
    if previous_code_blob is not None:
        previous_code = previous_code_blob.get_as_str()
    #generate the code
    code = await code_completion(
        state.code_spec.task_description, 
        None, 
        state.code_spec.data_examples,
        state.code_spec.input_spec, 
        state.code_spec.output_spec, 
        previous_code, 
        state.code_errors)
    code = strip_code(code)
    logger.info("generated code (stripped):")
    logger.info("=========================================")
    logger.info(code)
    logger.info("=========================================")
    #save the code
    test_node.makeb("generated.py").set_as_str(code)
    #add an entry point for the resolver
    ctx.core.makeb("wit_code_test").set_as_str("/code_test:generated:entry")
    #move state machine forward
    ctx.outbox.add_new_msg(ctx.actor_id, "test", mt="test")
    ctx.outbox.add_new_msg(ctx.agent_id, f"Testing: {state.name}", mt="thinking")

@app.message("test")
async def on_test_message(msg:InboxMessage, state:CoderState, ctx:MessageContext):
    logger.info("on_test_message")
    #load the code
    try:
        #use the resolver to load the code and any modules it might be referencing
        resolver = CoreResolver(ctx.store)
        entry_func = await resolver.resolve(ctx.core.get_as_object_id(), "wit_code_test", is_required=True)
        logger.info(f"resolved entry func: {entry_func}")
    except Exception as e:
        logger.info(f"syntax error, trying to resolve the function: {e}", exc_info=True)
        if state.code_tries >= state.code_tries_max:
            ctx.outbox.add_new_msg(ctx.actor_id, "fail", mt="fail")
        else:
            state.code_errors = str(e)
            ctx.outbox.add_new_msg(ctx.actor_id, "code", mt="code")
        return
    
    if state.code_spec.input_examples is not None and state.code_spec.input_examples != []:
        logger.info("running tests")
        test_errors = []
        for test_description in state.code_spec.input_examples:
            #generate the test data
            logger.info("test description: %s", test_description)
            test_input = await function_call_completion(
                "entry", 
                state.code_spec.task_description, 
                state.code_spec.input_spec, 
                test_description)
            logger.info("test input: %s", test_input)
            store_wrapper = StoreWrapper(ctx.store)
            function_kwargs = {}
            function_kwargs['input'] = test_input
            function_kwargs['store'] = store_wrapper
            try:
                logger.info("test kwargs: %s", function_kwargs)
                func_wrapper = _Wrapper(entry_func)
                output = await func_wrapper(**function_kwargs)
                logger.info("test output: %s", output)
            except Exception as e:
                logger.info(f"error trying to execute the generated function: {e}", exc_info=True)
                if str(e) not in test_errors:
                    test_errors.append(str(e))
        if len(test_errors) > 0:
            logger.info("test errors: %s", test_errors)
            if state.code_tries >= state.code_tries_max:
                logger.info("too many tries, will fail; tries: %s, max: %s", state.code_tries, state.code_tries_max)
                ctx.outbox.add_new_msg(ctx.actor_id, "fail", mt="fail")
            else:
                state.code_errors = "\n".join(test_errors)
                logger.info("will try again")
                ctx.outbox.add_new_msg(ctx.actor_id, "code", mt="code")
            return
        
    #the testing succeeded, move forward
    ctx.outbox.add_new_msg(ctx.actor_id, "deploy", mt="deploy")
    ctx.outbox.add_new_msg(ctx.agent_id, f"Deploying: {state.name}", mt="thinking")

@app.message("deploy")
async def on_deploy_message(msg:InboxMessage, state:CoderState, core:Core, outbox:Outbox, actor_id:ActorId):
    logger.info("on_deploy_message")
    state.code_errors = None # clear any errors since we succeeded now
    #copy the tested code into the main code node, making it deployed
    code:BlobObject = await core.get_path("code_test/generated.py")
    code_node = await core.gett("code")
    code_node.makeb("generated.py").set_from_blob(code)
    #add an entry point for the resolver
    core.makeb("wit_code").set_as_str("/code:generated:entry")
    #if this is a job, move on to the execute state
    if state.job_execution is not None:
        logger.info("this is a job, moving to execute state")
        outbox.add_new_msg(actor_id, state.job_execution, mt="execute")
    notify_all(state, outbox, CodeDeployed(code=code.get_as_str()), mt="code_deployed")

@app.message("execute")
async def on_execute_message(exec:CodeExecution, state:CoderState, core:Core, outbox:Outbox, actor_id:ActorId, store:ObjectStore):
    logger.info("on_execute_message")
    #use the resolver to load the code and any modules it might be referencing
    resolver = CoreResolver(store)
    try:
        entry_func = await resolver.resolve(core.get_as_object_id(), "wit_code", is_required=True)
    except Exception as e:
        logger.info("error trying to resolve the deployed function:", e)
        return
    
    #if the input arguments were just described, convert them to a function call
    if exec.input_arguments is None and exec.input_description is not None:
        logger.info("generating input arguments from input description: %s", exec.input_description)
        exec.input_arguments = await function_call_completion(
                "entry", 
                state.code_spec.task_description, 
                state.code_spec.input_spec, 
                exec.input_description)
        logger.info("generated input arguments: %s", exec.input_arguments)

    if exec.input_arguments is None:
        logger.info("no input arguments or description provided, cannot execute")
        return
    
    store_wrapper = StoreWrapper(store)
    function_kwargs = {}
    function_kwargs['input'] = exec.input_arguments
    function_kwargs['store'] = store_wrapper
    try:
        func_wrapper = _Wrapper(entry_func)
        output = await func_wrapper(**function_kwargs)
        logger.info("execute output:", output)
        notify_all(state, outbox, CodeExecuted(input_arguments=exec.input_arguments, output=output), mt="code_executed")
    except Exception as e:
        logger.info("error trying to execute the deployed function: %s", e)
        return

@app.message("fail")
async def on_fail_message(msg:InboxMessage, state:CoderState, ctx:MessageContext):
    logger.info("on_fail_message")
    notify_all(state, ctx.outbox, CodeFailed(errors=state.code_errors), mt="code_failed")

