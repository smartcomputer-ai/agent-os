import asyncio
import json
import logging
import uvicorn
from starlette.exceptions import HTTPException
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import Response, PlainTextResponse, JSONResponse, RedirectResponse
from starlette.routing import Route
from sse_starlette.sse import EventSourceResponse
from grit import *
from wit import *
from runtime import Runtime

# First version of implementing a HTTP API and web server that can be used to interact with individual actors.
# It utilizes the Starlette framework (https://www.starlette.io/).
#
# The web server two set of APIs:
# 1) query Grit objects
# 2)interact with actors, sending messages to them, listeing to new messages (via SSE), and executing queries
# 
# This is mostly a prototype and needs much more refinement.
#
# See the 'app' method in the beginning for the routes that are supported.

logger = logging.getLogger(__name__)

class WebServer:
    __AGENT_ID_PARAM = "agent_id"
    __ACTOR_ID_PARAM = "actor_id"
    __ACTOR_ID_QUERY_PARAM = "actor-id"
    __OBJECT_ID_PARAM = "object_id"
    __REFERENCE_PARAM = "ref"
    __QUERY_NAME_PARAM = "query_name"
    __QUERY_PATH_PARAM = "query_path"
    __WEB_REF_PARAM = "web_ref"
    __WEB_PATH_PARAM = "web_path"

    def __init__(self, runtime:Runtime):
        self.runtime = runtime
        self.object_store = runtime.store
        self.references = runtime.references

    def app(self) -> Starlette:
        grit_url_prefix = f"/ag/{{{self.__AGENT_ID_PARAM}}}/grit"
        wit_url_prefix  = f"/ag/{{{self.__AGENT_ID_PARAM}}}/wit"
        web_url_prefix  = f"/ag/{{{self.__AGENT_ID_PARAM}}}/web"
        routes = [
            Route('/', self.get_root),
            Route('/ag', self.agents_get_all),
            #grit routes
            Route(f"{grit_url_prefix}/refs", self.grit_get_refs),
            Route(f"{grit_url_prefix}/refs/{{{self.__REFERENCE_PARAM}}}", self.grit_get_ref),
            Route(f"{grit_url_prefix}/objects/{{{self.__OBJECT_ID_PARAM}}}", self.grit_get_object),
            #wit routes
            Route(f"{wit_url_prefix}/actors", self.wit_get_actors),
            Route(f"{wit_url_prefix}/actors/{{{self.__ACTOR_ID_PARAM}}}/inbox", self.wit_get_inbox),
            Route(f"{wit_url_prefix}/actors/{{{self.__ACTOR_ID_PARAM}}}/inbox", self.wit_post_inbox, methods=['POST']),
            Route(f"{wit_url_prefix}/actors/{{{self.__ACTOR_ID_PARAM}}}/outbox", self.wit_get_outbox),
            Route(f"{wit_url_prefix}/actors/{{{self.__ACTOR_ID_PARAM}}}/query/{{{self.__QUERY_NAME_PARAM}}}", self.wit_query),
            Route(f"{wit_url_prefix}/actors/{{{self.__ACTOR_ID_PARAM}}}/query/{{{self.__QUERY_NAME_PARAM}}}/{{{self.__QUERY_PATH_PARAM}:path}}", 
                  self.wit_query),
            Route(f"{wit_url_prefix}/messages-sse", self.wit_get_messages_sse),
            #web routes
            Route(f"{web_url_prefix}", self.web_get_ref),
            Route(f"{web_url_prefix}/{{{self.__WEB_REF_PARAM}}}", self.web_get_ref),
            Route(f"{web_url_prefix}/{{{self.__WEB_REF_PARAM}}}/{{{self.__WEB_PATH_PARAM}:path}}", self.web_get_ref),
        ]
        return Starlette(routes=routes, debug=True)

    async def run(self, port:int=5000, watch_dir:str=None):
        if port is None:
            port = 5000
        config = uvicorn.Config(app=self.app(), loop="asyncio", port=port, log_level="info")
        if(watch_dir is not None):
            config.reload = True
            config.reload_dirs = [watch_dir]
        self.server = uvicorn.Server(config)
        await self.server.serve()
    
    def stop(self):
        if(self.server is not None):
            self.server.should_exit = True

    def was_force_exited(self) -> bool:
        return self.server is not None and self.server.force_exit is True

    #=========================
    # Route handlers
    #=========================
    async def get_root(self, request:Request):
        return PlainTextResponse('Wit API')

    async def agents_get_all(self, request:Request):
        assert request.method == "GET"
        #in this setup, the API will only ever return a single agent id
        agent_id = self.runtime.agent_id
        agent_name = self.runtime.agent_name
        logger.info(f"Agent name: {agent_name}")
        logger.info(f"Agent id: {agent_id}")
        return JSONResponse({ agent_name: agent_id.hex()})

    async def grit_get_refs(self, request:Request):
        assert request.method == "GET"
        self.__validate_agent_id(request)
        refs = await self.references.get_all()
        return JSONResponse({ref:object_id.hex() for ref, object_id in refs.items()})
    
    async def grit_get_ref(self, request:Request):
        assert request.method == "GET"
        self.__validate_agent_id(request)
        ref = request.path_params[self.__REFERENCE_PARAM]
        object_id = await self.references.get(ref)
        if(object_id is None):
            raise HTTPException(status_code=404, detail=f"Reference ({ref}) not found")
        return JSONResponse({ref:object_id.hex()})

    async def grit_get_object(self, request:Request):
        assert request.method == "GET"
        self.__validate_agent_id(request)
        object_id = self.__validate_object_id(request)
        object = await self.object_store.load(object_id)
        if(object is None):
            raise HTTPException(status_code=404, detail=f"Object ({object_id.hex()}) not found")
        if is_blob(object):
            return self.__blob_object_to_response(object)
        else:
           return self. __other_object_to_response(object)

    def __blob_object_to_response(self, blob:Blob) -> Response:
        #try to deduce the content type from the blob headers
        if blob.headers is not None and len(blob.headers) > 0:
            if "Content-Type" in blob.headers:
                return Response(blob.data, media_type=blob.headers["Content-Type"])
            elif "ct" in blob.headers:
                if(blob.headers["ct"] == "b"):
                    return Response(blob.data, media_type="application/octet-stream")
                elif(blob.headers["ct"] == "s"):
                    return Response(blob.data, media_type="text/plain")
                elif(blob.headers["ct"] == "j"):
                    return Response(blob.data, media_type="application/json")
        else:
            try:
                blob_str = blob.data.decode('utf-8')
                try:
                    blob_json = json.loads(blob_str)
                    return Response(blob_json, media_type="application/json")
                except ValueError:
                    return Response(blob_str, media_type="text/plain")
            except (UnicodeDecodeError):
                return Response(blob.data, media_type="application/octet-stream")

    def __other_object_to_response(self, object) -> Response:
        if is_blob(object):
            raise Exception("Blob object not supported, use __blob_object_to_response")
        elif is_message(object):
            return JSONResponse(dict(object))
        elif is_step(object):
            return JSONResponse(dict(object))
        elif is_tree(object):
            return JSONResponse({k: v.hex() for k, v in object.items()})
        elif is_mailbox(object):
            return JSONResponse({k.hex(): v.hex() for k, v in object.items()})
        else:
            raise TypeError("Unknown object type")

    async def wit_get_actors(self, request:Request):
        assert request.method == "GET"
        self.__validate_agent_id(request)
        actor_ids = self.runtime.get_actors()
        return JSONResponse([actor_id.hex() for actor_id in actor_ids])

    async def wit_get_inbox(self, request:Request):
        assert request.method == "GET"
        self.__validate_agent_id(request)
        actor_id = await self.__validate_actor_id(request)
        inbox = await self.runtime.get_actor_inbox(actor_id)
        return JSONResponse({k.hex(): v.hex() for k, v in inbox.items()})
    
    async def wit_get_outbox(self, request:Request):
        assert request.method == "GET"
        self.__validate_agent_id(request)
        actor_id = await self.__validate_actor_id(request)
        outbox = await self.runtime.get_actor_outbox(actor_id)
        return JSONResponse({k.hex(): v.hex() for k, v in outbox.items()})
    
    async def wit_post_inbox(self, request:Request):
        assert request.method == "POST"
        agent_id = self.__validate_agent_id(request)
        actor_id = await self.__validate_actor_id(request)
        request_body_bytes = await request.body()
        if(len(request_body_bytes) == 0):
            raise HTTPException(status_code=400, detail="Request body must not be empty")
        blob_headers = {}
        if('Content-Type' in request.headers):
            ct = request.headers['Content-Type']
            #compress the content type to the short version used for bytes, strings, or json
            if(ct == 'application/json'):
                blob_headers['ct'] = 'j'
            elif(ct == 'text/plain'):
                blob_headers['ct'] = 's'
            elif(ct == 'application/octet-stream'):
                blob_headers['ct'] = 'b'
            else:
                #otherwise, just use the full content type
                blob_headers['Content-Type'] = ct
        message_headers = {}
        if('AOS-Message-Type' in request.headers):
            message_headers['mt'] = request.headers['AOS-Message-Type']
        else:
            message_headers['mt'] = 'web'
        logger.debug(f"message headers: {message_headers}")
        #conceptually, this is adding a new message to the inbox of the actor,
        # but internally the runtime treats an injected message as if its an outbox message,
        # i.e., a message being sent by an actor (which used the outbox)
        msg = OutboxMessage.from_new(
            actor_id, 
            BlobObject(Blob(blob_headers, request_body_bytes)))
        msg.headers = message_headers
        message_id = await self.runtime.inject_message(msg)
        return Response(
            content=message_id.hex(), 
            media_type='text/plain', 
            status_code=201, 
            headers={'Location': self.__get_object_id_path(agent_id, message_id)})

    async def wit_query(self, request:Request):
        assert request.method == "GET"
        self.__validate_agent_id(request)
        actor_id = await self.__validate_actor_id(request)
        query_name = request.path_params.get(self.__QUERY_NAME_PARAM)
        query_path = request.path_params.get(self.__QUERY_PATH_PARAM)
        if request.method == "GET":
            # create a contect from the query sting
            #conver query_params, which is a ImmutableMultiDict, to a dict, converting multi entries to a list
            query_context = {}
            for k, v in request.query_params.multi_items():
                if k in query_context:
                    query_context[k].append(v)
                else:
                    query_context[k] = [v]
            query_context = BlobObject.from_json(query_context).get_as_blob()
        else:
            #todo: allow PUT to upload a bigger context for a query
            query_context = None            
        # use the query executor to try to run the query
        query_result = await self.runtime.query_executor.run(actor_id, query_name, query_context)
        if(query_result is None):
            raise HTTPException(status_code=404, detail=f"Query ({query_name}) not found")
        if(is_blob(query_result)):
            #there should be no query path, if the result is a blob
            if(query_path is not None):
                raise HTTPException(status_code=400, detail=f"Path not supported for blob query results, do not specify a path beyond {query_name}")
            return self.__blob_object_to_response(query_result)
        elif(is_tree(query_result)):
            # as long as there is a path, descend the tree
            if(query_path is None):
                query_path  = "/"
            tree_obj = TreeObject(self.runtime.store, query_result)
            #split the path and descend the tree if there are multiple levels
            path_parts = query_path.split('/')
            path_parts = [part for part in path_parts if len(part) > 0]
            if(len(path_parts) == 0):
                return self.__other_object_to_response(query_result)
            while(len(path_parts) > 1):
                sub_tree = await tree_obj.get(path_parts[0])
                if(sub_tree is None):
                    raise HTTPException(status_code=404, detail=f"Path part ({path_parts[0]} in {query_path}) not found")
                #todo: brittle test
                if(type(sub_tree).__name__ != "TreeObject"):
                    raise HTTPException(status_code=400, detail=f"Path part ({path_parts[0]} in {query_path}) is not a tree")
                tree_obj = sub_tree
                path_parts = path_parts[1:]
            #assume the last part of the tree is a blob
            last_object = await tree_obj.get(path_parts[0])
            if(last_object is None):
                raise HTTPException(status_code=404, detail=f"Path part ({path_parts[0]} in {query_path}) not found")
            if(type(last_object).__name__ == "BlobObject"):
                return self.__blob_object_to_response(last_object.get_as_blob())
            else:
                return self.__other_object_to_response(last_object.get_as_tree())
        else:
            raise HTTPException(status_code=500, 
                detail=f"Query result ({query_name}) is not a blob or tree object, the query endpoint only supports serving blob and tree objects")

    async def wit_get_messages_sse(self, request:Request):
        self.__validate_agent_id(request)

        include_content = request.query_params.get('content', 'false').lower() == 'true'
        message_type_filters = request.query_params.getlist('mt')
        if(len(message_type_filters) == 0):
            message_type_filters = None
        logger.info(f"message type filters: {message_type_filters}")
        request.headers.get('Last-Event-ID')

        async def subscribe_to_messages():
            mailbox_updates_queue:asyncio.Queue[tuple[ActorId, ActorId, MessageId]]
            try:
                with self.runtime.subscribe_to_messages() as mailbox_updates_queue:
                    while True:
                        #todo: add time out to handle cancel, etc
                        mailbox_update = await mailbox_updates_queue.get()
                        logger.debug(f"got mailbox update: {mailbox_update}")
                        #take an empty message as a signal that the runtime is stoping
                        if(mailbox_update is None):
                            break
                        message_id = mailbox_update[2]
                        message_id_str = message_id.hex()
                        message:Message = await self.runtime.store.load(message_id)
                        if message.headers is not None and "mt" in message.headers:
                            message_type = message.headers["mt"]
                        else:
                            message_type = "message"
                        #if the subscriber defined a message type filter, skip messages that don't match
                        if(message_type_filters is not None and message_type not in message_type_filters):
                            logger.debug("skipping msg:", message_type)
                            continue                            
                        sse_data = { 
                            "sender_id": mailbox_update[0].hex(),
                            "reciever_id": mailbox_update[1].hex(),
                            "message_id": message_id_str
                        }
                        if(include_content):
                            message_content = await self.runtime.store.load(message.content)
                            if is_blob(message_content):
                                sse_data["content"] = message_content.data.decode('utf-8')
                            elif is_tree(message_content):
                                sse_data["content"] = json.dumps(message_content)
                            else:
                                raise HTTPException(status_code=500, detail="Message content is not a blob or tree object.")
                        yield { 
                            #the data field is required by the SSE spec
                            # and it needs to be a valid JSON string inside data
                            "id": message_id_str,
                            "event": message_type,
                            "data": json.dumps(sse_data)
                        }
                        
            except asyncio.CancelledError as e:
                logger.info(f"Disconnected from client (via refresh/close) {request.client}")
                # todo: do any other cleanup, if any
                raise e
        
        return EventSourceResponse(
            subscribe_to_messages(),
            headers={'Cache-Control': "public, max-age=3200"},
            )

    async def web_get_ref(self, request:Request):
        assert request.method == "GET"
        self.__validate_agent_id(request)
        ref = request.path_params.get(self.__WEB_REF_PARAM)
        path = request.path_params.get(self.__WEB_PATH_PARAM)
        #todo: consider just setting it to "root" and serve without redirect
        if(ref is None):
            return RedirectResponse(url=f"/ag/{request.path_params[self.__AGENT_ID_PARAM]}/web/root")
        #try to get the web reference (must be set in the references store)
        full_ref = f"web/{ref}"
        object_id = await self.runtime.references.get(full_ref)
        if(object_id is None):
            raise HTTPException(status_code=404, detail=f"Web reference ({full_ref}) not found. Please set it in the references store.")
        #get the object and serve it
        object = await self.object_store.load(object_id)
        if(object is None):
            raise HTTPException(status_code=404, detail=f"Object ({object_id.hex()}) not found")
        if(is_blob(object)):
            if(path is not None):
                raise HTTPException(status_code=400, detail=f"Path not supported for blob objects, do not specify a path beyond {ref}")
            return self.__blob_object_to_response(object)
        elif(is_tree(object)):
            if(path is None):
                return RedirectResponse(url=f"/ag/{request.path_params[self.__AGENT_ID_PARAM]}/web/{ref}/index")
            tree_obj = TreeObject(self.runtime.store, object, object_id)
            #split the path and descend the tree if there are multiple levels
            path_parts = path.split('/')
            path_parts = [part for part in path_parts if len(part) > 0]
            while(len(path_parts) > 1):
                sub_tree = await tree_obj.get(path_parts[0])
                if(sub_tree is None):
                    raise HTTPException(status_code=404, detail=f"Path part ({path_parts[0]} in {path}) not found")
                #todo: brittle test
                if(type(sub_tree).__name__ != "TreeObject"):
                    raise HTTPException(status_code=400, detail=f"Path part ({path_parts[0]} in {path}) is not a tree")
                tree_obj = sub_tree
                path_parts = path_parts[1:]
            #assume the last part of the tree is a blob
            blob_obj = await tree_obj.get(path_parts[0])
            if(blob_obj is None):
                raise HTTPException(status_code=404, detail=f"Path part ({path_parts[0]} in {path}) not found")
            if(type(blob_obj).__name__ != "BlobObject"):
                raise HTTPException(status_code=400,
                    detail=f"Path part ({path_parts[0]} in {path}) is not a blob, expected a blob at the end of the path")
            return self.__blob_object_to_response(blob_obj.get_as_blob())
        else:
            raise HTTPException(status_code=400, 
                detail=f"Object ({object_id.hex()}) is not a blob or tree object, the web endpoint only supports serving blob and tree objects")

    def __validate_agent_id(self, request:Request) -> bytes:
        if(self.__AGENT_ID_PARAM not in request.path_params):
            raise HTTPException(status_code=400, detail="Agent id not set")
        agent_id_or_name_str = request.path_params[self.__AGENT_ID_PARAM]
        if(not is_object_id_str(agent_id_or_name_str)):
            if(agent_id_or_name_str == self.runtime.agent_name):
                return self.runtime.agent_id
            raise HTTPException(status_code=404, detail=f"Agent id ({agent_id_or_name_str}) not found") 
        else:
            agent_id = bytes.fromhex(agent_id_or_name_str)
            if(agent_id == self.runtime.agent_id):
                return agent_id
            raise HTTPException(status_code=404, detail=f"Agent id ({agent_id_or_name_str}) not found") 
    
    async def __validate_actor_id(self, request:Request) -> bytes:
        if(self.__ACTOR_ID_PARAM not in request.path_params):
            raise HTTPException(status_code=400, detail="Actor id not set")
        actor_id_or_ref_str = request.path_params[self.__ACTOR_ID_PARAM]
        if(not is_object_id_str(actor_id_or_ref_str)):
            actor_id = await self.references.get(ref_actor_name(actor_id_or_ref_str))
            if(actor_id is not None):
                return actor_id
            raise HTTPException(status_code=400, detail=f"Invalid actor id ({actor_id_or_ref_str})")
        else:
            actor_id = bytes.fromhex(actor_id_or_ref_str)
            if(not self.runtime.actor_exists(actor_id)):
                raise HTTPException(status_code=404, detail=f"Actor id ({actor_id_or_ref_str}) not found")
            return actor_id
    
    def __validate_object_id(self, request:Request) -> bytes:
        if(self.__OBJECT_ID_PARAM not in request.path_params):
            raise HTTPException(status_code=400, detail="Object id not set")
        object_id_str = request.path_params[self.__OBJECT_ID_PARAM]
        if(not is_object_id_str(object_id_str)):
            raise HTTPException(status_code=400, detail=f"Invalid object id ({object_id_str})")
        return bytes.fromhex(object_id_str)
    
    def __get_object_id_path(self, agent_id, object_id) -> str:
        if(isinstance(agent_id, bytes)):
            agent_id = agent_id.hex()
        if(isinstance(object_id, bytes)):
            object_id = object_id.hex()
        return f"/ag/{agent_id}/grit/objects/{object_id}"
