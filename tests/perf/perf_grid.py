import pickle
from src.grit.stores.memory import MemoryObjectStore, MemoryReferences
from src.wit import *
from src.runtime import *

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
N_TEST_MESSAGES = 50

count_init_messages = 0
count_grid_messages = 0

class TopWit(Wit):
    core_first_row_wits:list[ActorId] = None

    async def on_genesis_message(self, message:InboxMessage) -> None:
        #print(f"TopWit, on_genesis_message: I am {self.actor_id.hex()}")
        self.core_first_row_wits = []
        for i in range(N_COLUMNS):
            first_row_gen = GridWit.create_genesis_core(self.store)
            await set_args(first_row_gen, {"rows_remaining": N_ROWS, "column": i, "top_wit": self.actor_id.hex()})
            first_row_gen_msg = await self.send_genesis_message(first_row_gen)
            self.core_first_row_wits.append(first_row_gen_msg.recipient_id)

    async def on_message(self, message:InboxMessage) -> None:
        #print(f"TopWit, on_message: I am {self.actor_id.hex()}")
        content = (await message.get_content()).get_as_str()
        #init messages start with "init", responses start with "grid"
        if(content.startswith("init")):
            #print("init message")
            global count_init_messages
            count_init_messages += 1
            for grid_wit_id in self.core_first_row_wits:
                self.send_message(grid_wit_id, "grid")
        elif(content.startswith("grid")):
            global count_grid_messages
            count_grid_messages += 1
        else:
            print("Unknown message: "+content)


class GridWit(Wit):
    core_forward_messages_to:ActorId = None

    async def on_genesis_message(self, message:InboxMessage) -> None:
        #print(f"GridWit, on_genesis_message: I am {self.actor_id.hex()}")
        args = await get_args(self.core)
        rows_remaining = args["rows_remaining"]
        column = args["column"]
        top_wit_id = bytes.fromhex(args["top_wit"])
        if(rows_remaining > 0):
            next_row_gen = GridWit.create_genesis_core(self.store)
            await set_args(next_row_gen, {"rows_remaining": rows_remaining - 1, "column": column, "top_wit": args["top_wit"]})
            next_row_gen_msg = await self.send_genesis_message(next_row_gen)
            #since there are still more rows, when non-gen message arrives, forward to next grid wit
            self.core_forward_messages_to = next_row_gen_msg.recipient_id
        else:
            #forward message back to top wit
            self.core_forward_messages_to = top_wit_id

    async def on_message(self, message:InboxMessage) -> None:
        #print(f"GridWit, on_message: I am {self.actor_id.hex()}")
        args = await get_args(self.core)
        rows_remaining = args["rows_remaining"]
        column = args["column"]
        content = (await message.get_content()).get_as_str()
        content = content + f"\n-> {rows_remaining} x {column}: {self.actor_id.hex()}"
        self.send_message(self.core_forward_messages_to, content)


async def get_args(core:Core) -> dict:
    return ((await (await core.gett('data')).getb("args")).get_as_json())

async def set_args(core:Core, args:dict) -> None:
    (await (await core.gett('data')).getb("args")).set_as_json(args)

async def perf_grid_run(store:ObjectStore, refs:References) -> None:
    print(f"Will run perf_grid with {N_COLUMNS} columns and {N_ROWS} rows, and {N_TEST_MESSAGES} test messages.") 

    resolver = ExternalResolver(store)
    TopWit.register_wit(resolver)
    GridWit.register_wit(resolver)

    runtime = Runtime(store, refs, "test", resolver)

    running_task = asyncio.create_task(runtime.start())
    print("Runtime started")
    #genesis
    print("Injecting genesis message...")
    gen_message = await TopWit.create_genesis_message(store)
    await runtime.inject_message(gen_message)
    await runtime.wait_until_running()
    
    print(f"Injecting other {N_TEST_MESSAGES} messages...")
    for i in range(N_TEST_MESSAGES):
        await runtime.inject_message(OutboxMessage.from_new(gen_message.recipient_id, f"init {i}"))
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
    print(f"total processed in the grid: {N_COLUMNS * N_ROWS * N_TEST_MESSAGES}")
    assert count_init_messages == N_TEST_MESSAGES
    assert count_grid_messages == N_COLUMNS * N_TEST_MESSAGES