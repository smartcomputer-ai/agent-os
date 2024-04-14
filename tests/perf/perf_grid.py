import faulthandler
from src.wit import *
from src.runtime import *
faulthandler.enable()
# Create a grid of wits, like this:
#        TopWit     <--------|
#       /    \   \           |
#     GridW  GridW  GridW    |
#       |      |      |      |
#     GridW  GridW  GridW    |
#       |      |      |      |
#     GridW  GridW  GridW    |
#       |      |      |      |
#     GridW  GridW  GridW    |
#       |      |      |      |
#       ----------------------
VERBOSE = False
N_COLUMNS = 50
N_ROWS = 50
N_TEST_MESSAGES = 200

count_init_messages = 0
count_grid_messages = 0


#=================================================================================
# Top Wit
#=================================================================================
async def create_top_actor(
        store:ObjectStore,
        ) -> OutboxMessage:
    core = Core.from_external_wit_ref(store, wit_ref="top_wit")
    genesis_msg = await OutboxMessage.from_genesis(store, core)
    return genesis_msg

class TopState(WitState):
    core_first_row_wits:list[ActorId] = None

top_wit = Wit()

@top_wit.genesis_message
async def on_genesis_message(message:InboxMessage, ctx:MessageContext, state: TopState) -> None:
    print(f"TopWit, on_genesis_message: I am {ctx.actor_id.hex()}")
    state.core_first_row_wits = []
    for i in range(N_COLUMNS):
        args = {"rows_remaining": N_ROWS, "column": i, "top_wit": ctx.actor_id.hex()}
        first_row_gen_msg = await create_grid_actor(ctx.store, args)
        state.core_first_row_wits.append(first_row_gen_msg.recipient_id)
        ctx.outbox.add(first_row_gen_msg)

@top_wit.message("init")
async def on_init_message(message:InboxMessage, ctx:MessageContext, state: TopState) -> None:
    print(f"TopWit, on_init_message: I am {ctx.actor_id.hex()}")
    global count_init_messages
    count_init_messages += 1
    for grid_wit_id in state.core_first_row_wits:
        ctx.outbox.add_new_msg(grid_wit_id, "grid", mt="grid")

@top_wit.message("grid")
async def on_grid_callback_message(message:InboxMessage, ctx:MessageContext) -> None:
    print(f"TopWit, on_grid_callback_message: I am {ctx.actor_id.hex()}")
    global count_grid_messages
    count_grid_messages += 1

#=================================================================================
# Grid Wit
#=================================================================================
async def create_grid_actor(
        store:ObjectStore,
        args:dict, 
        ) -> OutboxMessage:
    core = Core.from_external_wit_ref(store, wit_ref="grid_wit")

    await set_args(core, args)
    genesis_msg = await OutboxMessage.from_genesis(store, core)
    return genesis_msg


class GridState(WitState):
    core_forward_messages_to:ActorId = None

grid_wit = Wit()

@grid_wit.genesis_message
async def on_grid_genesis_message(message:InboxMessage, ctx:MessageContext, state:GridState) -> None:
    #print(f"GridWit, on_genesis_message: I am {ctx.actor_id.hex()}")
    args = await get_args(ctx.core)
    rows_remaining = args["rows_remaining"]
    column = args["column"]
    top_wit_id = bytes.fromhex(args["top_wit"])
    if(rows_remaining > 0):
        new_args = {"rows_remaining": rows_remaining - 1, "column": column, "top_wit": args["top_wit"]}
        next_row_gen_msg = await create_grid_actor(ctx.store, new_args)
        ctx.outbox.add(next_row_gen_msg)
        #since there are still more rows, when non-gen message arrives, forward to next grid wit
        state.core_forward_messages_to = next_row_gen_msg.recipient_id
    else:
        #forward message back to top wit
        state.core_forward_messages_to = top_wit_id

@grid_wit.message("grid")
async def on_grid_message(message:InboxMessage, ctx:MessageContext, state:GridState) -> None:
    #print(f"GridWit, on_grid_message: I am {ctx.actor_id.hex()}")
    args = await get_args(ctx.core)
    rows_remaining = args["rows_remaining"]
    column = args["column"]
    content = (await message.get_content()).get_as_str()
    content = content + f"\n-> {rows_remaining} x {column}: {ctx.actor_id.hex()}"
    ctx.outbox.add_new_msg(state.core_forward_messages_to, content, mt="grid")


async def get_args(core:Core) -> dict:
    return ((await (await core.gett('data')).getb("args")).get_as_json())

async def set_args(core:Core, args:dict) -> None:
    (await (await core.gett('data')).getb("args")).set_as_json(args)

async def perf_grid_run(store:ObjectStore, refs:References) -> None:
    print(f"Will run perf_grid with {N_COLUMNS} columns and {N_ROWS} rows, and {N_TEST_MESSAGES} test messages.") 

    resolver = ExternalResolver(store)
    resolver.register("top_wit", top_wit)
    resolver.register("grid_wit", grid_wit)

    runtime = Runtime(store, refs, "test", resolver)

    running_task = asyncio.create_task(runtime.start())
    print("Runtime started")
    #genesis
    print("Injecting genesis message...")
    gen_message = await create_top_actor(store)
    await runtime.inject_message(gen_message)
    await runtime.wait_until_running()
    
    print(f"Injecting other {N_TEST_MESSAGES} messages...")
    for i in range(N_TEST_MESSAGES):
        await runtime.inject_message(OutboxMessage.from_new(gen_message.recipient_id, f"init {i}", mt="init"))
        await asyncio.sleep(0.001)

    last_print = 0
    while(count_grid_messages < N_COLUMNS * N_TEST_MESSAGES):
        await asyncio.sleep(0.2)
        if(last_print < count_grid_messages):
            print(f" -> processed so far: count_grid_messages: {count_grid_messages}")
            last_print += 10
        
    #stop
    runtime.stop()
    await running_task

    print(f"count_init_messages: {count_init_messages}")
    print(f"count_grid_messages: {count_grid_messages}")
    total_grid_messages = N_COLUMNS * N_ROWS * N_TEST_MESSAGES
    print(f"total processed in the grid: {total_grid_messages}")
    assert count_init_messages == N_TEST_MESSAGES
    assert count_grid_messages == N_COLUMNS * N_TEST_MESSAGES