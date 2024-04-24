import logging
import os
import random
import lmdb
from aos.grit import *
from aos.runtime.core.root_executor import bootstrap_root_actor_bytes, agent_id_from_point
from aos.runtime.store import grit_store_pb2
from aos.runtime.store import agent_store_pb2
from aos.runtime.store import agent_store_pb2

_GRIT_ID_LEN = 32

logger = logging.getLogger(__name__)

# TODO: refactor to avoid protobuf objects in the API, should be Servicer impl only
#       most APIs here take two or three params and return a single value, so no need to use those object here

class LmdbBackend:
    def __init__(self, store_path:str, writemap:bool=False):
        self.store_path = store_path
        self._resizing = False
        os.makedirs(self.store_path, exist_ok=True)
        self.env = lmdb.Environment(
            store_path, 
            max_dbs=5, 
            # writemap=True is what makes lmdb FAST (about 10x faster than if its False), 
            # BUT it makes the DB file as big as the mapsize (at least on some file systems). 
            # Plus, it comes with fewer safety guarantees.
            # See: https://lmdb.readthedocs.io/en/release/#writemap-mode
            writemap=writemap, 
            metasync=False, 
            # Flush write buffers asynchronously to disk
            # if wirtemap is False, this is ignored
            map_async=True,
            # 10 MB, is ignored if it's bigger already
            map_size=1024*1024*10, 
            )

    #=========================================================
    # Env API
    # Docs: https://lmdb.readthedocs.io/en/release/#environment-class
    #=========================================================
    def get_env(self) -> lmdb.Environment:
        return self.env
    
    def get_agents_db(self) -> lmdb._Database:
        return self.env.open_db('agents'.encode('utf-8'))

    def get_object_db(self) -> lmdb._Database:
        return self.env.open_db('obj'.encode('utf-8'))
    
    def get_refs_db(self) -> lmdb._Database:
        return self.env.open_db('refs'.encode('utf-8'))
    
    def get_vars_db(self) -> lmdb._Database:
        return self.env.open_db('vars'.encode('utf-8'))
    
    def begin_agents_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_agents_db(), write=write, buffers=buffers)

    def begin_object_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_object_db(), write=write, buffers=buffers)

    def begin_refs_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_refs_db(), write=write, buffers=buffers)
    
    def begin_vars_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_vars_db(), write=write, buffers=buffers)
    

    def _resize(self) -> int:
        #TODO: is this safe? Do we need to lock in some way or other?
        # probably not, because it is only run in a single process
        self._resizing = True
        current_size = self.env.info()['map_size']
        if current_size > 1024*1024*1024*10: # 10 GB
            multiplier = 1.2
        elif current_size > 1024*1024*1024: # 1 GB
            multiplier = 1.5
        else: # under 1 GB
            multiplier = 3.0
        # must be rounded to next int! otherwise lmdb will segfault later (spent several hours on this)
        new_size = round(current_size * multiplier) 
        logger.info(f"Resizing LMDB map from {current_size/1024/1024} MB to {new_size/1024/1024} MB")
        self.env.set_mapsize(new_size)
        self._resizing = False
        return new_size

    def _ensure_agent(self, agent_id:ActorId):
        #load all the agents
        with self.begin_agents_txn(write=False) as txn:
            existing_point = txn.get(agent_id)
            if existing_point is None:
                raise ValueError(f"Agent with id '{agent_id.hex()}' does not exist.")

    #=========================================================
    # Grit Object Store API
    #=========================================================
    def store(self, request:grit_store_pb2.StoreRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        #check the object id
        object_id = get_object_id(request.data)
        if request.object_id is not None and not is_object_id_match(object_id, request.object_id):
            raise ValueError(f"object_id in request does not match object_id derived from request data.")
        object_key = _make_object_key(request.agent_id, object_id)
        try:
            with self.begin_object_txn() as txn:
                txn.put(object_key, request.data, overwrite=False)
        except lmdb.MapFullError:
            logger.warning(f"===> Resizing LMDB map... in obj store, (obj id: {object_id.hex()}) <===")
            self._resize()
            #try again
            with self.begin_object_txn() as txn:
                txn.put(object_key, request.data, overwrite=False)

        return None
    

    def load(self, request:grit_store_pb2.LoadRequest) -> grit_store_pb2.LoadResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        object_id = request.object_id
        if not is_object_id(object_id):
            raise ValueError(f"object_id is not a properly structured ObjectId: type '{type(object_id)}', len {len(object_id)}.")
        self._ensure_agent(request.agent_id)
        object_key = _make_object_key(request.agent_id, object_id)
        with self.begin_object_txn(write=False) as txn:
            data = txn.get(object_key, default=None)
        
        return grit_store_pb2.LoadResponse(
            agent_id=request.agent_id,
            object_id=object_id,
            data=data)
    

    #=========================================================
    # References Store API
    #=========================================================
    def set_ref(self, request:grit_store_pb2.SetRefRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        ref_key = _make_refs_key(request.agent_id, request.ref)
        with self.begin_refs_txn() as txn:
            txn.put(ref_key, request.object_id)

        return None


    def get_ref(self, request:grit_store_pb2.GetRefRequest)  -> grit_store_pb2.GetRefResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        ref_key = _make_refs_key(request.agent_id, request.ref)
        with self.begin_refs_txn(write=False) as txn:
            object_id = txn.get(ref_key, default=None)
        
        return grit_store_pb2.GetRefResponse(
            agent_id=request.agent_id,
            ref=request.ref,
            object_id=object_id)
    

    def get_refs(self, request:grit_store_pb2.GetRefsRequest) -> grit_store_pb2.GetRefsResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        search_key = request.agent_id
        if request.ref_prefix is not None:
            search_key = _make_refs_key(request.agent_id, request.ref_prefix)
        refs = {}
        with self.begin_refs_txn(write=False) as txn:
            cursor = txn.cursor()
            cursor.set_range(search_key)
            while _bytes_startswith(cursor.key(), search_key):
                _, ref = _parse_refs_key(cursor.key())
                refs[ref] = cursor.value()
                cursor.next()

        return grit_store_pb2.GetRefsResponse(
            agent_id=request.agent_id,
            refs=refs)
     
    #=========================================================
    # Agent CRUD/Management API
    #=========================================================  
    def get_agent(self, request:agent_store_pb2.GetAgentRequest) -> agent_store_pb2.GetAgentResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        if (not request.HasField("point") and not request.HasField("agent_id")):
            raise ValueError("either point or agent_id must be provided.")
        
        with self.begin_agents_txn(write=False) as txn:
            if request.agent_id:
                point_bytes = txn.get(request.agent_id)
                if point_bytes is not None:
                    return agent_store_pb2.GetAgentResponse(
                        exists=True,
                        agent_id=request.agent_id,
                        point=bytes_to_point(point_bytes))
            else:
                agent_id = txn.get(point_to_bytes(request.point))
                if agent_id is not None:
                    return agent_store_pb2.GetAgentResponse(
                        exists=True,
                        agent_id=agent_id,
                        point=request.point)
        return agent_store_pb2.GetAgentResponse(exists=False)


    def get_agents(self, request:agent_store_pb2.GetAgentsRequest) -> agent_store_pb2.GetAgentsResponse:
        agents:dict[int, bytes] = {}

        agents_db = self.get_agents_db()
        vars_db = self.get_vars_db()
        with self.env.begin(write=True) as txn:
            agents_cursor = txn.cursor(db=agents_db)
            agents_cursor.first()
            while agents_cursor.next():
                #since agents are stored both by point and agent_id, check that this is a point entry
                point_bytes = agents_cursor.key()
                if len(point_bytes) != 8:
                    continue
                
                point = bytes_to_point(point_bytes)
                agent_id:bytes = agents_cursor.value()

                #if filters are set, check if the agent matches the filter
                if request.var_filters is not None and len(request.var_filters) > 0:
                    for var_filter_key, var_filter_value in request.var_filters.items():
                        var_value = txn.get(_make_var_key(agent_id, var_filter_key), db=vars_db)
                        if var_value is None and var_filter_value is not None:
                            break
                        if var_filter_value != var_value.decode('utf-8'):
                            break
                    else:
                        #all filters matched
                        agents[point] = agent_id
                else:
                    #no filters
                    agents[point] = agent_id

        return agent_store_pb2.GetAgentsResponse(agents=agents)
    

    def create_agent(self, request:agent_store_pb2.CreateAgentRequest) -> agent_store_pb2.CreateAgentResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        
        #create the agent in the db
        agents_db = self.get_agents_db()
        object_db = self.get_object_db()
        refs_db = self.get_refs_db()
        vars_db = self.get_vars_db()
        with self.env.begin(write=True) as txn:

            if request.HasField("point"):
                point = request.point
            else:
                point = None
                logger.info("Generating new agent point.")
                range_start = pow(2,17)
                range_end = pow(2, 19)-1
                max_tries = 100000
                for _ in range(max_tries):
                    point = random.randint(range_start, range_end)
                    #make sure it doesn't exist already
                    existing_agent_id = txn.get(point_to_bytes(point), db=agents_db)
                    if existing_agent_id is None:
                        break
                    else:
                        point = None
                if point is None:
                    raise Exception(f"Failed to generate a new point in range ({range_start} - {range_end}) after {max_tries} tries.")
                
                logger.info(f"Generated agent point: {point}")

            #check if the agent already exists
            existing_agent_id = txn.get(point_to_bytes(point), db=agents_db)
            if existing_agent_id is not None:
                logger.warning(f"Agent point '{point}' already exists for agent id {existing_agent_id.hex()}, will return existing agent.")
                return agent_store_pb2.CreateAgentResponse(
                    agent_id=existing_agent_id,
                    point=point)
            
            logger.info(f"Creating agent for point '{point}'")

            #since the agent doesn't exist we need to create the root actor for that agent,
            # which determines the agent_id: the agent_id is the actor_id of the root actor
            # the "root actor" is a priviledged actor that represents the runtime and the agent itself
            #
            # creating the root actor is tricky because the db expects the agent_id to already exist,
            # but the the id is not known until all the objects and initial core exists for that root actor
            # however, the trick is to construct all relevant grit objects in memory, derrive the actor id from there
            # and then write all the objects to the db in a single transaction
            # there are some helper functions for this which are used here: agent_id_from_point, bootstrap_root_actor_bytes
            agent_id = agent_id_from_point(point)

            #create the agent (both ways, so they can be found by point or agent_id)
            #TODO: do this as a separte transaction above. there is a subble race condition now, where the same agent 
            #      can be created for the same point at the same time
            #      this really needs to be implemented as a two-phase commit
            if (not txn.put(point_to_bytes(point), agent_id, db=agents_db, overwrite=False) or
                not txn.put(agent_id, point_to_bytes(point), db=agents_db, overwrite=False)):
                raise Exception(f"Failed to create agent for point {point} and {agent_id.hex()}, it already existed.")

            last_obj_id = None
            for obj_id, obj_bytes in bootstrap_root_actor_bytes(point):            
                txn.put(
                    _make_object_key(agent_id, obj_id), 
                    obj_bytes, 
                    db=object_db)
                last_obj_id = obj_id

            #the last id from the iterator was the genesis step id
            gen_step_id = last_obj_id
            
            #set the initial references (step HEAD)
            txn.put(
                _make_refs_key(agent_id, ref_step_head(agent_id)), 
                gen_step_id, 
                db=refs_db)
            txn.put(
                _make_refs_key(agent_id, ref_root_actor()),
                agent_id,
                db=refs_db)

            return agent_store_pb2.CreateAgentResponse(
                agent_id=agent_id,
                point=point)
        

    def delete_agent(self, request:agent_store_pb2.DeleteAgentRequest) -> agent_store_pb2.DeleteAgentResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        if (not request.HasField("point") and not request.HasField("agent_id")):
            raise ValueError("either point or agent_id must be provided.")
        
        agents_db = self.get_agents_db()
        object_db = self.get_object_db()
        refs_db = self.get_refs_db()
        vars_db = self.get_vars_db()

        with self.env.begin(write=True) as txn:
            #get the agent id
            agent_id = request.agent_id
            point = request.point
            if agent_id is None:
                agent_id = txn.get(point_to_bytes(point), db=agents_db)
                #if the agent id is still None, then the agent does not exist
                if agent_id is None:
                    return agent_store_pb2.DeleteAgentResponse()
            else:
                #check that the agent point actually exists
                point_bytes = txn.get(agent_id, db=agents_db)
                if point_bytes is None:
                    return agent_store_pb2.DeleteAgentResponse()
                point = bytes_to_point(point_bytes)
            
            #delete all objects
            cursor = txn.cursor(db=object_db)
            cursor.set_range(agent_id)
            while _bytes_startswith(cursor.key(), agent_id):
                txn.delete(cursor.key())
                cursor.next()

            #delete all refs
            cursor = txn.cursor(db=refs_db)
            cursor.set_range(agent_id)
            while _bytes_startswith(cursor.key(), agent_id):
                txn.delete(cursor.key())
                cursor.next()

            #delete all vars
            cursor = txn.cursor(db=vars_db)
            cursor.set_range(agent_id)
            while _bytes_startswith(cursor.key(), agent_id):
                txn.delete(cursor.key())
                cursor.next()

            #delete the agent
            txn.delete(point_to_bytes(point), db=agents_db)
            txn.delete(agent_id, db=agents_db)
        
        return agent_store_pb2.DeleteAgentResponse()
    
    #=========================================================
    # Agent Var(iables) Store API
    #=========================================================
    def set_var(self, request:agent_store_pb2.SetVarRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        var_key = _make_var_key(request.agent_id, request.key)
        with self.begin_vars_txn() as txn:
            txn.put(var_key, request.value.encode('utf-8'))
        return None


    def get_var(self, request:agent_store_pb2.GetVarRequest)  -> agent_store_pb2.GetVarResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        var_key = _make_var_key(request.agent_id, request.key)
        with self.begin_vars_txn(write=False) as txn:
            value:bytes = txn.get(var_key, default=None)
        
        return agent_store_pb2.GetVarResponse(
            agent_id=request.agent_id,
            key=request.key,
            value=value.decode('utf-8'))

    def get_vars(self, request:agent_store_pb2.GetVarsRequest) -> agent_store_pb2.GetVarsResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        search_key = request.agent_id
        if request.key_prefix is not None:
            search_key = _make_var_key(request.agent_id, request.key_prefix)
        vars = {}
        with self.begin_vars_txn(write=False) as txn:
            cursor = txn.cursor()
            cursor.set_range(search_key)
            while _bytes_startswith(cursor.key(), search_key):
                _, key = _parse_var_key(cursor.key())
                vars[key] = cursor.value().decode('utf-8')
                cursor.next()

        return agent_store_pb2.GetVarsResponse(
            agent_id=request.agent_id,
            vars=vars)
    
    def delete_var(self, request:agent_store_pb2.DeleteVarRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        var_key = _make_var_key(request.agent_id, request.key)
        with self.begin_vars_txn() as txn:
            txn.delete(var_key)
        return None

#=========================================================
# Utils
#=========================================================
def _make_object_key(agent_id:ActorId, object_id:ObjectId) -> bytes:
    return agent_id + object_id

def _parse_object_key(key:bytes) -> tuple[ActorId, ObjectId]:
    return key[:_GRIT_ID_LEN], key[_GRIT_ID_LEN:]

def _make_refs_key(agent_id:ActorId, ref:str) -> bytes:
    return agent_id + ref.encode('utf-8')

def _parse_refs_key(key:bytes) -> tuple[ActorId, str]:
    return key[:_GRIT_ID_LEN], key[_GRIT_ID_LEN:].decode('utf-8')

def _make_var_key(agent_id:ActorId, key:str) -> bytes:
    return agent_id + key.encode('utf-8')

def _parse_var_key(key:bytes) -> tuple[ActorId, str]:
    return key[:_GRIT_ID_LEN], key[_GRIT_ID_LEN:].decode('utf-8')

def _bytes_startswith(haystack:bytes, needle:bytes) -> bool:
    return haystack[:len(needle)] == needle

