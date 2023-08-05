from grit import *
from wit import *

app = Wit()

@app.message("gen")
async def on_gen_message():
    pass