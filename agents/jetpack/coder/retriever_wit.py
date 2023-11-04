import logging
from grit import *
from wit import *
from jetpack.messages import *
from jetpack.coder.retriever_completions import *
from jetpack.coder.coder_wit import create_coder_actor

#========================================================================================
# Setup & State
#========================================================================================
logger = logging.getLogger(__name__)

async def create_retriever_actor(
        ctx:MessageContext,
        request:CodeRequest|None=None,
        forward_to:ActorId|None=None,
        ) -> ActorId:
    state = RetrieverState()
    state.code_request = request
    state.forward_to = forward_to
    state.notify.add(ctx.actor_id)
    return await create_actor_from_prototype_with_state(
        ctx.prototype_actors["retriever"], 
        state, 
        ctx.request_response, 
        ctx.store)

class RetrieverState(WitState):
    code_request:CodeRequest|None = None
    locations:list[str]|None = None
    retrieval_coders:dict[ActorId,str]|None = None
    retrieved_data:dict[str,str]|None = None
    notify:set[ActorId] = set()
    forward_to:ActorId|None = None

#========================================================================================
# Wit
#========================================================================================
app = Wit()

@app.genesis_message
async def on_genesis_message(msg:InboxMessage, core:Core, state:RetrieverState, outbox:Outbox, actor_id:ActorId):
    logger.info("on_genesis_message")
    # if code specs were provided, move right to the plan
    # and send message to move on to the plan state 
    if state.code_request is not None:
        outbox.add_new_msg(actor_id, "plan", mt="plan")

@app.message("request")
async def on_request_message(request:CodeRequest, msg:InboxMessage, state:RetrieverState, outbox:Outbox, actor_id:ActorId):
    logger.info("on_request_message")
    state.code_request = request
    state.notify.add(msg.sender_id)
    outbox.add_new_msg(actor_id, "plan", mt="plan")

@app.message("plan")
async def on_plan_message(msg:InboxMessage, state:RetrieverState, ctx:MessageContext):
    logger.info("on_plan_message")

    # see if any data needs to be retrieved that needs to feature in the code generation later
    retrievals = await retrieve_completion(
        state.code_request.task_description,
        state.code_request.input_examples,
        )
    if retrievals is not None:
        logger.info("retrievals:", retrievals)
        state.locations = retrievals
        ctx.outbox.add_new_msg(ctx.actor_id, "retrieve", mt="retrieve")
        ctx.outbox.add_new_msg(ctx.agent_id, "Retrieving Data", mt="thinking")
        return
    else:
        logger.info("no retrievals needed")
        state.locations = None
        state.retrieved_data = None
        ctx.outbox.add_new_msg(ctx.actor_id, "complete", mt="complete")

@app.message("retrieve")
async def on_retrieve_message(msg:InboxMessage, state:RetrieverState, ctx:MessageContext):
    logger.info("on_retrieve_message")
    state.retrieval_coders = {}
    if state.retrieved_data is None:
        state.retrieved_data = {}
    for location in state.locations:
        #check if the data was already retrieved
        if location in state.retrieved_data:
            logger.info("data already retrieved for location:", location)
            continue
        #create a code actor to retrieve the data
        task_description = f"""For context, we are writing code for the following task:
        ```
        {state.code_request.task_description}
        ```
        However, before we can write the code, we need to understand what the structure of the data is at the following location:
        ```
        {location}
        ```
        Whatever the format of the data at that location, we need to retrieve it and return it as a string (`contents`).
        Do not take any inputs to the function, just use the location as a variable inside the code.
        To retrieve the schema, do not use the openpyxl library, just use requests.
        """

        retrieve_spec = CodeSpec(
            task_description=task_description,
            input_spec=CodeSpec.empty_inputoutput_spec(),
            output_spec=json.loads('{"properties": {"contents": {"title": "Contents of Location", "type": "string"}}, "required": ["contents"], "type": "object"}'),
            input_examples=[json.dumps(CodeSpec.empty_inputoutput_spec())]
            )
        retrieve_job = CodeExecution(
            input_arguments=json.loads('{"type": "object", "properties": {}}'),
            input_description=None,
        )
        coder_id = await create_coder_actor(
            ctx, 
            f"retrieve: {location}",
            retrieve_spec,
            retrieve_job)
        state.retrieval_coders[coder_id] = location
        state.retrieved_data[location] = None
    
    #if no retrievals coders were needed, move on to the completed state
    if len(state.retrieval_coders) == 0:
        ctx.outbox.add_new_msg(ctx.actor_id, "complete", mt="complete")

@app.message("complete")
async def on_complete_message(msg:InboxMessage, state:RetrieverState, outbox:Outbox, actor_id:ActorId):
    logger.info("on_complete_message")
    if state.forward_to is not None:
        spec = CodeSpec(
            task_description=state.code_request.task_description,
            input_examples=state.code_request.input_examples,
            input_spec=None,
            output_spec=None,
            data_examples=state.retrieved_data,
        )
        outbox.add_new_msg(state.forward_to, spec, mt="spec")


#========================================================================================
# Coder Callbacks
#========================================================================================
@app.message("code_executed")
async def on_message_code_executed(exec:CodeExecuted, ctx:MessageContext, state:RetrieverState) -> None:
    logger.info("received callback: code_executed")
    
    #see if there is coder associated with this message
    if state.retrieval_coders is None:
        return
    if ctx.message.sender_id in state.retrieval_coders:
        location = state.retrieval_coders[ctx.message.sender_id]
        state.retrieved_data[location] = exec.output['contents']
        logger.info("retrieved data for location:", location, state.retrieved_data[location])
        state.retrieval_coders.pop(ctx.message.sender_id)
        if len(state.retrieval_coders) == 0:
            ctx.outbox.add_new_msg(ctx.actor_id, "complete", mt="complete")



def notify_all(state:RetrieverState, outbox:Outbox, msg:any, mt:str|None=None):
    for actor_id in state.notify:
        outbox.add_new_msg(actor_id, msg, mt=mt)