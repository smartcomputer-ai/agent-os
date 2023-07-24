from __future__ import annotations
from typing import Callable
from grit import *
from .data_model import *
from .data_model_utils import *
from .wit_routers import _WitMessageRouter, _WitQueryRouter, DecoratedCallable
from .wut import _WutGenerator

#===================================================================================================
# Main Wit API Object
#
# Used to decorate functions (e.g. with '@wit.message("test")') to handle wit messages or queries.
# To see how this is actually implemented look in wit_routers.py
#===================================================================================================
class Wit:
    """The Wit API is used to decorate functions that are then called to handle messages or a query."""
    def __init__(self,
            fail_on_unhandled_message:bool=False,
            fail_on_unhandled_query:bool=False,
            generate_wut_query:bool=True,
            ) -> None:
        self._wit_message_router = _WitMessageRouter(fail_on_unhandled=fail_on_unhandled_message)
        self._wit_query_router = _WitQueryRouter(fail_on_unhandled=fail_on_unhandled_query)
        if(generate_wut_query):
            self._wut_generator = _WutGenerator(self._wit_message_router, self._wit_query_router, register_with_query_router=True)

    async def __call__(self, *args, **kwargs):
        # Figure out, based on the args, if this is a state transition (wit) or query (wit_query) call
        # look at the positional arguments:
        # 1) wit expects: (step_id:StepId|None, new_messages:Mailbox)
        # 2) wit query expects: (step_id:StepId, query_name:str, context:Blob|None)
        # update wits are just a normal wit, so like #1
        if(len(args) == 2):
            return await self._wit_message_router.run(*args, **kwargs)
        elif(len(args) == 3):
            return await self._wit_query_router.run(*args, **kwargs)
        else:
            raise Exception(f"Invalid number of arguments to wit: {len(args)}")
    
    #Wit Messages
    def run_wit(self, func, /) -> Callable[[], DecoratedCallable]:
        return self._wit_message_router.register_run_handler(func)

    def message(self, /, message_type:str, *, more_struff:str=None) -> Callable[[DecoratedCallable], DecoratedCallable]:
        def decorator(func:DecoratedCallable) -> DecoratedCallable:
            return self._wit_message_router.register_message_handler(message_type, func)
        return decorator
    
    def genesis_message(self, func, /) -> Callable[[], DecoratedCallable]:
        return self._wit_message_router.register_message_handler("genesis", func)

    def update_message(self, func, /) -> Callable[[], DecoratedCallable]:
        return self._wit_message_router.register_message_handler("update", func)
    
    #Queries
    def run_query(self, func, /) -> Callable[[], DecoratedCallable]:
        return self._wit_query_router.register_run_handler(func)
    
    def query(self, /, query_name:str, *, more_struff:str=None) -> Callable[[DecoratedCallable], DecoratedCallable]:
        def decorator(func:DecoratedCallable) -> DecoratedCallable:
            return self._wit_query_router.register_query_handler(query_name, func)
        return decorator
    
    def wut_query(self, func, /) -> Callable[[], DecoratedCallable]:
        return self._wit_query_router.register_query_handler("wut", func)
    