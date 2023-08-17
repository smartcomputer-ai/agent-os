from __future__ import annotations
from grit import *
from .data_model import *
from .data_model_utils import *
from .wit_routers import _WitMessageRouter, _WitQueryRouter, _WitMessageWrapper, _NamedQueryWrapper

# The 'wut' query is just a convention to implement a query name that reurns information about the wit.
# It is not required, but it is a good idea to implement it.
#
# Since the routers have all the information about the message and query handlers, we can just plug into
# the routers and generate that information automatically. Kind of like how FastApi generates OpenAPI specs. 
# 
# The current implementation is just a proof of concept.

class _WutGenerator:
    def __init__(self, 
            message_router:_WitMessageRouter|None, 
            query_router:_WitQueryRouter|None,
            register_with_query_router:bool=True,):
        self.message_router = message_router
        self.query_router = query_router
        # see if it should register itself with the query router
        if self.query_router is not None and register_with_query_router:
            self.query_router.register_query_handler("wut", self.generate_wut)

    def generate_wut(self) -> BlobObject:
        """Generate a wut file from the registered wits."""
        # Of course, this is just a demo, this should really produce a proper OpenAPI type of spec,
        # use pydanic schemas, and so on
        # TODO: implement this properly
        wut = {}
        if(self.message_router):
            wit_handlers = {}
            wut["messages"] = wit_handlers
            wrapper: _WitMessageWrapper
            for message_type, wrapper in self.message_router._wit_message_handlers.items():
                if(message_type == "genesis" or message_type == "update"):
                    continue
                if wrapper.input_param is None:
                    wit_handlers[message_type] = str(type(InboxMessage))
                else:
                    wit_handlers[message_type] = str(wrapper.input_param.annotation)
        if(self.query_router):
            query_handlers = {}
            wut["queries"] = query_handlers
            wrapper: _NamedQueryWrapper
            for query_name, wrapper in self.query_router._query_handlers.items():
                if wrapper.input_param is None:
                    query_handlers[query_name] = ""
                else:
                    query_handlers[query_name] = str(wrapper.input_param.annotation)
        return BlobObject.from_json(wut)