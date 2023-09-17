
from __future__ import annotations
import inspect
from typing import Any, Callable, TypeVar
from dataclasses import dataclass
from itertools import islice
from pydantic import BaseModel
from grit import *
from .data_model import *
from .data_model_utils import *
from .errors import InvalidWitException, InvalidMessageException, QueryError
from .wit_state import WitState

# The classes are mostly used internally to wrap user defined functions and route wit messages to the
# correct message handler.

DecoratedCallable = TypeVar("DecoratedCallable", bound=Callable[..., Any])

#===================================================================================================
# Context Classes
#===================================================================================================
@dataclass(frozen=True)
class MessageContext():
    message:InboxMessage|None
    messages:list[InboxMessage]|None
    inbox:Inbox
    outbox:Outbox
    core:Core
    step_id:StepId
    actor_id:ActorId
    agent_id:ActorId
    store:ObjectStore

@dataclass(frozen=True)
class QueryContext():
    query_name:str
    query_args:Blob|None
    query_args_json:dict|None
    inbox:Inbox
    outbox:Outbox
    core:Core
    step_id:StepId
    actor_id:ActorId
    agent_id:ActorId
    loader:ObjectLoader

#===================================================================================================
# Function Wrapper Util
#===================================================================================================
class _Wrapper():
    """Wraps a user defined function and analizes the signature to determine how to call it."""
    def __init__(self, func:Callable) -> None:
        self.func = func
        unwrapped_func = inspect.unwrap(func)
        #strings need to be evaluated for types that are loaded from a core (see core_loader.py) to work
        self.sig = inspect.signature(unwrapped_func, eval_str=True)
        params = self.sig.parameters
        if not callable(unwrapped_func):
            raise InvalidWitException("Decorator can only be added to functions.")
        self.is_method = 'self' in params and list(params.keys()).index('self') == 0
        self.is_class_method = 'cls' in params and list(params.keys()).index('cls') == 0
        if self.is_method or self.is_class_method:
            raise InvalidWitException("Decorator can only be used on functions, not on instance or class methods.")
        self.is_async = inspect.iscoroutinefunction(unwrapped_func)
        self.has_kwargs = any(param.kind == param.VAR_KEYWORD for param in params.values())
        self.state_params = {} # there can be several, loaded from the core
        for name, param in params.items():
            if param.annotation is not inspect.Parameter.empty and inspect.isclass(param.annotation):
                try:
                    if issubclass(param.annotation, WitState):
                        self.state_params[name] = param.annotation
                except TypeError:
                    pass
                    
    async def __call__(self, *args, **kwargs):
        #print("handlder call:", self.func.__name__, args, kwargs)
        (new_args, new_kwargs) = self._match_func_signature(*args, **kwargs)
        #print("handlder call new args:", self.func.__name__, new_args, new_kwargs)
        if(self.is_async):
            return await self.func(*new_args, **new_kwargs)
        else:
            #todo: run in thread pool if configured to do so
            return await asyncio.to_thread(self.func, *new_args, **new_kwargs)

    def _match_func_signature(self, *args, **kwargs):
        params = self.sig.parameters
        # if it has kwargs, then just call the function as is
        if self.has_kwargs:
            return (args, kwargs,)
        # otherwise, construct a kwargs that matches the function signature
        else:
            # get the first n args, where n is the number of positional args
            # (this is a bit of a hack, but it works)
            args_to_pass = list(islice(args, len(params)))
            #print(f"args_to_pass: {args_to_pass}")
            # construct a dict of kwargs that match the function signature
            kwargs_to_pass = {k: v for k, v in kwargs.items() if k in params}
            return ((*args_to_pass,), kwargs_to_pass,)
    
    async def _load_state_params(self, kwargs:dict):
        #see if state needs to be loaded
        if len(self.state_params) > 0:
            #print("loading state")
            #print("state params:", self.state_params)
            core:Core = kwargs.get('core', None)
            if core is None:
                raise InvalidWitException("To load state requires a 'core' parameter.")
            for param_name, state_type in self.state_params.items():
                if(not issubclass(state_type, WitState)):
                    raise InvalidWitException(f"Invalid state type: {state_type} must inherit from {WitState}.")
                state:WitState = state_type()
                await state._load_from_core(core)
                kwargs[param_name] = state

    async def _persist_state_params(self, kwargs:dict):
        #see if state needs to be persisted
        if len(self.state_params) > 0:
            #print("persisting state")
            #print("state params:", self.state_params)
            core:Core = kwargs.get('core', None)
            if core is None:
                raise InvalidWitException("To persist state requires a 'core' parameter.")
            for param_name, state in kwargs.items():
                if param_name in self.state_params:
                    if(not isinstance(state, WitState)):
                        raise InvalidWitException(f"Invalid state type: {state} must inherit from {WitState}.")
                    await state._persist_to_core(core)

#===================================================================================================
# Wit Message Routing & Handling
#
# A with is defined as follows:
# def wit(step:StepId, new_messages:Mailbox, **kwargs) -> StepId:
#
# It takes a grit step as input (the id, that is) and creates a new step as output.
# But the runtime also provides a list of new messages, which is really a wit is about:
# Take a set of new messages and merge it into its state by creating a new step.
#
# However, writing wit transtion functions like this would be extremely cumbersome.
# What we want to do instead is use some higher-level abstractions, such as the ones
# defined in data_model.py:
#
# def wit(inbox:Inbox, outbox:Outbox, core:Core, **kwargs) -> None:
#
# So the wrappers implemented here supply the core, inbox, and outbox as arguments
# and convert back and forth between the low-level and high-level abstractions. 
#===================================================================================================
class _WitWrapper(_Wrapper):
    def __init__(self, func:Callable) -> None:
        super().__init__(func)
        self.context_param = None
        for param in self.sig.parameters.values():
            # if param.annotation is not inspect.Parameter.empty:
            #     print("_WitWrapper: param:", self.func.__name__, param.name, param.annotation,
            #            type(param.annotation), inspect.isclass(param.annotation))
            if param.annotation is not inspect.Parameter.empty and inspect.isclass(param.annotation) and issubclass(param.annotation, MessageContext):
                self.context_param = param

    async def __call__(self, *args, **kwargs):
        await self._load_state_params(kwargs)
        step_id = await super().__call__(*args, **kwargs)
        await self._persist_state_params(kwargs)
        return step_id

class _WitMessageWrapper(_WitWrapper):
    """Wraps a wit message function and enables conversion of an InboxMessage to more usable, 
    deserialized message content types."""
    def __init__(self, func:Callable) -> None:
        super().__init__(func)
        params = self.sig.parameters
        self.input_param = None 
        #check if the function requires a conversion of the input message to a different type
        if(len(params) > 0):
            param = list(params.values())[0]
            allowed_target_types = (BaseModel, TreeObject, BlobObject, str, dict,)
            if (param.annotation is not inspect.Parameter.empty 
                and inspect.isclass(param.annotation) 
                and issubclass(param.annotation, allowed_target_types)
                ):
                self.input_param = param
                #print("detected converted input param:", param)
    
    async def __call__(self, *args, **kwargs):
        # before calling the parent function, see if the first argument needs to be converted to a different type
        if self.input_param is not None:
            # convert the parameter in the first position
            # the case where this is an InboxMessage is constrained by the setup in the _WitMessageRouter (see below)
            input_value = args[0]
            if not isinstance(input_value, InboxMessage):
                raise Exception(f"Input parameter was not an InboxMessage, but a {type(input_value)}.")
            converted_value = await self._convert_message_to_input_param(input_value)
            args = (converted_value, *args[1:])
            # if the input_param is not called 'message' or 'msg', add the original InboxMessage (input_value) to the kwargs, 
            # so that the function can access it if it needs to
            if self.input_param.name != 'message':
                kwargs['message'] = input_value
            if self.input_param.name != 'msg':
                kwargs['msg'] = input_value
        # now call the parent function, which actually calls the user defined function
        return await super().__call__(*args, **kwargs)

    async def _convert_message_to_input_param(self, message:InboxMessage) -> Any:
        if self.input_param is None:
            raise Exception("No input_param set.")
        # convert the input_value to the input_param type, here 'target_type'
        target_type = self.input_param.annotation
        if issubclass(target_type, TreeObject):
            converted = await message.get_content()
            if(not isinstance(converted, TreeObject)):
                raise InvalidMessageException(f"Input message '{message.message_id.hex()}' content was not a tree, but a {type(converted)}.")
            return converted
        else:
            converted = await message.get_content()
            if(not isinstance(converted, BlobObject)):
                raise InvalidMessageException(f"Input message '{message.message_id.hex()}' content was not a blob, but a {type(converted)}.")
            if issubclass(target_type, BlobObject):
                return converted
            elif issubclass(target_type, BaseModel):
                return converted.get_as_model(target_type)
            elif issubclass(target_type, str):
                return converted.get_as_str()
            elif issubclass(target_type, dict):
                return converted.get_as_json()
        raise InvalidMessageException(f"Could not convert to desired input parameter '{self.input_param.name}' value, "+
                                      f"from message '{message.message_id.hex()}' to parameter type '{target_type.__name__}'.")


class _WitMessageRouter:
    def __init__(self, fail_on_unhandled=False) -> None:
        self.fail_on_unhandled = fail_on_unhandled
        self._wit_run = None
        self._wit_message_handlers = {}
    
    def register_run_handler(self, func:Callable) -> _WitWrapper:
        if self._wit_run is not None:
            raise InvalidWitException("Cannot register more than one run handler.")
        if len(self._wit_message_handlers) > 0:
            raise InvalidWitException(
                f"Cannot register run handler after registering individual message handlers: {self._wit_message_handlers.keys()}.")
        self._wit_run = _WitWrapper(func)
        return self._wit_run

    def register_message_handler(self, message_type:str, func:Callable) -> _WitMessageWrapper:
        if self._wit_run is not None:
            raise InvalidWitException(
                f"Cannot register individual message handler for message type '{message_type}' after registering run handler.")
        if message_type in self._wit_message_handlers:
            raise InvalidWitException(
                f"Cannot register more than one handler for message type '{message_type}'.")
        self._wit_message_handlers[message_type] = _WitMessageWrapper(func)
        return self._wit_message_handlers[message_type]

    async def run(self, last_step_id:StepId, new_inbox:Mailbox, **kwargs) -> StepId:
        last_step_id, new_inbox = self._enforece_required_args(*(last_step_id, new_inbox,))
        actor_id, _, object_store = self._enforce_required_kwargs(**kwargs)
        inbox, outbox, core = await load_step(object_store, actor_id, last_step_id, new_inbox)
        # build new kwargs
        
        # if the user provided the run handler, use that one
        if(self._wit_run is not None):
            new_kwargs = self._build_new_kwargs(kwargs, self._wit_run, last_step_id, inbox, outbox, core, object_store)
            await self._wit_run(**new_kwargs)
        # otherwise handle the messages here
        else:
            #reading the inbox actually updates the inbox by marking the messages as "read" (kinda like an email inbox)
            messages = await inbox.read_new()
            for message in messages:
                if message.mt is not None:
                    # see if there is a message handler for that message type (mt)
                    handler = self._wit_message_handlers.get(message.mt)
                    if handler is not None:
                        new_kwargs = self._build_new_kwargs(
                            kwargs, 
                            handler, 
                            last_step_id, 
                            inbox, 
                            outbox, 
                            core, 
                            object_store, 
                            messages, 
                            message)
                        await handler(*(message,), **new_kwargs)
                        continue
                if self.fail_on_unhandled:
                    # dont fail on unhandeled genesis or update messages (because they are usually automatically handled)
                    if message.mt is not None and (message.mt == 'genesis' or message.mt == 'update'):
                        continue
                    raise InvalidMessageException(f"Unhandled message type '{message.mt}' in message '{message.message_id.hex()}'.")
        # persist the step
        new_step_id = await persist_step(object_store, actor_id, last_step_id, inbox, outbox, core)
        return new_step_id

    def _enforece_required_args(self, *args) -> tuple[StepId, Mailbox]:
        if len(args) != 2:
            raise Exception(f"Wit functions must take exactly two arguments (step_id, new_messages), but it was {len(args)}.")
        last_step_id:StepId|None = args[0]
        if not is_object_id(last_step_id) and last_step_id is not None:
            raise TypeError("The first argument to a wit function must be a StepId or None.")
        new_inbox:Mailbox = args[1]
        if not is_mailbox(new_inbox):
            raise TypeError("The second argument to a wit function must be a mailbox dict.")
        return (last_step_id, new_inbox)
    
    def _enforce_required_kwargs(self, **kwargs) -> tuple[ActorId, ActorId, ObjectStore]:
        actor_id:ActorId = kwargs.get('actor_id')
        if(not isinstance(actor_id, ActorId)):
            raise Exception("The 'actor_id' argument must be provided and must be an ActorId.")
        agent_id:ActorId = kwargs.get('agent_id')
        if(not isinstance(agent_id, ActorId)):
            raise Exception("The 'agent_id' argument must be provided and must be an ActorId.")
        object_store:ObjectStore = kwargs.get('object_store')
        if object_store is None:
            object_store:ObjectStore = kwargs.get('store')
        if(not isinstance(object_store, ObjectStore)):
            raise Exception("The 'object_store' or 'store' argument must be provided and must be an ObjectStore.")
        return (actor_id, agent_id, object_store)
    
    def _build_new_kwargs(self, kwargs:dict, wrapper:_WitWrapper, 
                          step_id:StepId, inbox, outbox, core, store:ObjectStore, 
                          messages=None, message=None):
        kwargs = kwargs.copy()
        kwargs.setdefault('inbox', inbox)
        kwargs.setdefault('outbox', outbox)
        kwargs.setdefault('core', core)
        kwargs.setdefault('store', store)
        kwargs.setdefault('object_store', store)
        if wrapper.context_param is not None:
            ctx = MessageContext(
                message=message,
                messages=messages,
                step_id=step_id,
                inbox=inbox,
                outbox=outbox,
                core=core,
                store=store,
                actor_id=kwargs.get('actor_id'),
                agent_id=kwargs.get('agent_id'),
            )
            kwargs[wrapper.context_param.name] = ctx
        return kwargs

#===================================================================================================
# Query Routing & Handling
#
# The query interface allows developers to define reading operatines side-by-side with wit functions.
# Any query is not strictly related to a wit, but a query does take as input a specific step of an actor.
# Beyond that, a query can do whatever it wants, expect write data. Specifically, it cannot create a new step.
# So, queries are usualy defined by the developer of the wit and who understand the internal structure 
# of the core and mailboxes of a particular wit.
#
# The universal query interface looks like this:
# def wit_query(step:StepId, query_name:str, query_args:Blob|None) -> Blob | Tree | Error:
#
# But for a developer, it would be easier to define a query like this:
# def my_query_name(**kwargs) -> Blob | Tree | Error:
# Where the qwargs contain inbox, outbox, core, and the unrolled context into individual key-value pairs (if it is JSON).
# So, this is what this API provides.
# Note, that in practice, the Error is not returned as a value, but thrown as an exception.
#===================================================================================================
ValidQueryReturnValues = None | BlobId | TreeId | Tree | Blob | BlobObject | TreeObject | str | bytes | dict, BaseModel

class _QueryWrapper(_Wrapper):
    def __init__(self, func:Callable) -> None:
        super().__init__(func)
        self.context_param = None
        for param in self.sig.parameters.values():
            if param.annotation is not inspect.Parameter.empty and inspect.isclass(param.annotation):
                if issubclass(param.annotation, QueryContext):
                    self.context_param = param

    async def __call__(self, *args, **kwargs):
        #print("calling query:", kwargs)
        await self._load_state_params(kwargs)
        return await super().__call__(*args, **kwargs)

class _NamedQueryWrapper(_QueryWrapper):
    def __init__(self, func:Callable) -> None:
        super().__init__(func)
        params = self.sig.parameters
        self.input_param = None 
        #check if a specific query implementation needs to have the 'query_args' converted to a different type
        if(len(params) > 0):
            param = list(params.values())[0]
            allowed_target_types = (BaseModel, BlobObject,)
            if (param.annotation is not inspect.Parameter.empty 
                and inspect.isclass(param.annotation) 
                and issubclass(param.annotation, allowed_target_types)):
                self.input_param = param
                #print("detected converted input param:", param)
    
    async def __call__(self, *args, **kwargs):
        #unless there is a converted input param the query should not be called with positional arguments
        if(len(args) > 0):
            raise Exception("Named queries do not take positional arguments.")
        # before calling the parent function, see if the 'query_args' argument needs to be converted
        if self.input_param is not None:
            input_value = kwargs.get('query_args')
            if(input_value is not None):
                #must be blob
                if not isinstance(input_value, Blob):
                    raise QueryError("Since a conversion is required and 'query_args' have been provided, 'query_args' must be a Blob.")
                converted_value = await self._convert_query_args_to_input_param(input_value)
                args = (converted_value,)
        # now call the parent function, but this time with a positional argument (if it was found)
        return await super().__call__(*args, **kwargs)

    async def _convert_query_args_to_input_param(self, query_args:Blob) -> Any:
        if self.input_param is None:
            raise Exception("No input_param set.")
        # convert the input_value to the input_param type, here 'target_type'
        target_type = self.input_param.annotation
        converted = BlobObject(query_args)
        if issubclass(target_type, BlobObject):
            return converted
        elif issubclass(target_type, BaseModel):
            return converted.get_as_model(target_type)
        raise QueryError(f"Could not convert to desired input parameter '{self.input_param.name}' value, "+
                         f"from query_args blob to parameter type '{target_type.__name__}'.")

class _WitQueryRouter:
    def __init__(self, fail_on_unhandled=False) -> None:
        self.fail_on_unhandled = fail_on_unhandled
        self._query_run = None
        self._query_handlers = {}
    
    def register_run_handler(self, func:Callable) -> _QueryWrapper:
        if self._query_run is not None:
            raise InvalidWitException("Cannot register more than one run handler.")
        if len(self._query_handlers) > 0:
            raise InvalidWitException(
                f"Cannot register run handler after registering individual query handlers: {self._query_handlers.keys()}.")
        self._query_run = _QueryWrapper(func)
        return self._query_run

    def register_query_handler(self, query_name:str, func:Callable) -> _NamedQueryWrapper:
        if self._query_run is not None:
            raise InvalidWitException(f"Cannot register individual query handler for '{query_name}' after registering run handler.")
        if query_name in self._query_handlers:
            raise InvalidWitException(f"Cannot register more than one handler for query name '{query_name}'.")
        self._query_handlers[query_name] = _NamedQueryWrapper(func)
        return self._query_handlers[query_name]

    async def run(self, step_id:StepId, query_name:str, query_args:Blob|None, **kwargs) -> Tree|Blob:
        step_id, query_name, query_args = self._enforece_required_args(*(step_id, query_name, query_args,))
        _, _, object_loader = self._enforce_required_kwargs(**kwargs)
        # load the step data
        (inbox, outbox, core) = await load_step_from_last(object_loader, step_id, None)
        # extend the kwargs so most combinations work
        # if the user provided the run handler, use that one
        if(self._query_run is not None):
            new_kwargs = self._build_new_kwargs(kwargs, self._query_run, step_id, query_name, query_args, inbox, outbox, core, object_loader)
            query_result = await self._query_run(*(query_name,), **new_kwargs)
        # otherwise handle the query here
        else:
            handler = self._query_handlers.get(query_name)
            if handler is not None:
                new_kwargs = self._build_new_kwargs(kwargs, handler, step_id, query_name, query_args, inbox, outbox, core, object_loader)
                query_result = await handler(**new_kwargs)
            elif self.fail_on_unhandled:
                raise QueryError(f"Unhandled query_name '{query_name}'.")
            else:
                query_result = None
        #convert the result to a tree or blob
        converted_result = await self._convert_result_to_tree_or_blob(query_result, object_loader)
        return converted_result

    
    def _enforece_required_args(self, *args) -> tuple[StepId, str, Blob|None]:
        if len(args) != 3:
            raise Exception(f"Wit query functions must take exactly three arguments (step_id, query_name, context), but it was {len(args)}.")
        step_id:StepId|None = args[0]
        if not is_object_id(step_id):
            raise TypeError("The first argument to a wit query function must be a StepId.")
        query_name:Mailbox = args[1]
        if not isinstance(query_name, str):
            raise TypeError("The second argument to a wit query function must be a str, the query name.")
        query_args:Blob|None = args[2]
        if query_args is not None and not is_blob(query_args):
            raise TypeError("The third argument to a wit query function must be a Blob or None.")
        return (step_id, query_name, query_args)
    
    def _enforce_required_kwargs(self, **kwargs) -> tuple[ActorId, ActorId, ObjectLoader]:
        actor_id:ActorId = kwargs.get('actor_id')
        if(not isinstance(actor_id, ActorId)):
            raise Exception("The 'actor_id' argument must be provided and must be an ActorId.")
        agent_id:ActorId = kwargs.get('agent_id')
        if(not isinstance(agent_id, ActorId)):
            raise Exception("The 'agent_id' argument must be provided and must be an ActorId.")
        possible_loader_keys = ['loader', 'object_loader', 'store', 'object_store']
        loader = None
        for key in possible_loader_keys:
            if key in kwargs:
                loader = kwargs[key]
                break
        if loader is None:
            raise Exception("Could not find a loader in the kwargs.")
        if(not isinstance(loader, ObjectLoader)):
            raise Exception("The loader must be ObjectLoader.")
        return (actor_id, agent_id, loader)

    def _build_new_kwargs(self, kwargs:dict, wrapper:_QueryWrapper, 
                          step_id:StepId, query_name:str, query_args:Blob|None, 
                          inbox, outbox, core,
                          loader:ObjectLoader):
        kwargs = kwargs.copy()
        kwargs.setdefault('inbox', inbox)
        kwargs.setdefault('outbox', outbox)
        kwargs.setdefault('core', core)
        kwargs.setdefault('query_args', query_args)
        kwargs.setdefault('loader', loader)
        kwargs.setdefault('object_loader', loader)

        #parse the arguments into json if the content type is json
        query_args_json = None
        if(query_args is not None and query_args.headers is not None):
            if (('ct' in query_args.headers and query_args.headers['ct'] == 'j') 
                or ('Content-Type' in query_args.headers and query_args.headers['Content-Type'] == 'application/json')
                ):
                try:
                    query_args_json = json.loads(query_args.data)
                except Exception as e:
                    raise QueryError(f"The query_args content type is JSON, but was not able to parse it: {e}") from e

        # create a context object, if the target function has a context parameter  
        if wrapper.context_param is not None:
            kwargs[wrapper.context_param.name] = QueryContext(
                query_name=query_name,
                query_args=query_args,
                query_args_json=query_args_json,
                step_id=step_id,
                inbox=inbox,
                outbox=outbox,
                core=core,
                loader=loader,
                actor_id=kwargs['actor_id'],
                agent_id=kwargs['agent_id'])
        
        #finally, add the json args to the kwargs, so they can be accessed direcly by name
        if(isinstance(query_args_json, dict)):
            for key, value in query_args_json.items():
                # do not allow json keys to override any of the existing kwargs
                if(key not in kwargs):
                    kwargs[key] = value
        return kwargs

    async def _convert_result_to_tree_or_blob(self, result:ValidQueryReturnValues, loader:ObjectLoader) -> Tree|Blob:
        if result is None:
            return None
        elif is_object_id(result):
            obj = await loader.load(result)
            #object id could point to any type of object, but queries only support trees and blobs
            if not is_blob(obj) and not is_tree(obj):
                raise QueryError(f"Invalid return type from wit query function: {type(obj)} for object_id '{result.hex()}'.")
            return obj
        elif is_tree(result):
            return result
        elif is_blob(result):
            return result
        elif isinstance(result, BlobObject):
            return result.get_as_blob()
        elif isinstance(result, TreeObject):
            #if the tree was modified, this will throw an exception
            return result.get_as_tree()
        elif isinstance(result, str):
            return BlobObject.from_str(result).get_as_blob()
        elif isinstance(result, bytes):
            return BlobObject.from_bytes(result).get_as_blob()
        elif isinstance(result, dict):
            return BlobObject.from_json(result).get_as_blob()
        elif isinstance(result, BaseModel):
            return BlobObject.from_json(result).get_as_blob()
        else:
            raise Exception(f"Invalid return type from wit query function: {type(result)}")
