import logging
import os
import lmdb
from aos.grit import *
from aos.runtime.store import grit_store_pb2
from aos.runtime.store import secret_store_pb2
from aos.runtime.crypto.did_key import generate_did

_GRIT_ID_LEN = 32

logger = logging.getLogger(__name__)

# TODO: refactor to avoid protobuf objects in the API, should be Servicer impl only
#       most APIs here take two or three params and return a single value, so no need to use those object here

class LmdbBackend:
    def __init__(self, store_path:str, writemap:bool=False):
        self.store_path = store_path
        self._resizing = False
        self._agents = None
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
    
    def get_secrets_db(self) -> lmdb._Database:
        return self.env.open_db('secrets'.encode('utf-8'))
    
    def begin_agents_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_agents_db(), write=write, buffers=buffers)

    def begin_object_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_object_db(), write=write, buffers=buffers)

    def begin_refs_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_refs_db(), write=write, buffers=buffers)
    
    def begin_secrets_txn(self, write=True, buffers=False) -> lmdb.Transaction:
        return self.env.begin(db=self.get_secrets_db(), write=write, buffers=buffers)
    

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
        if(self._agents is None):
            self._agents = set()
            #load all the agents
            with self.begin_agents_txn(write=False) as txn:
                cursor = txn.cursor()
                cursor.first()
                while cursor.next():
                    agent_id = cursor.value()
                    self._agents.add(agent_id)
        if not agent_id in self._agents:
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
            logger.warn(f"===> Resizing LMDB map... in obj store, (obj id: {object_id.hex()}) <===")
            self._resize()
            #try again
            with self.begin_object_txn() as txn:
                txn.put(object_key, request.data, overwrite=False)

        return None
    

    def load(self, request:grit_store_pb2.LoadRequest) -> grit_store_pb2.LoadResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        if(not is_object_id(request.object_id)):
            raise TypeError(f"object_id must be of type ObjectId, not '{type(request.object_id)}'.")
        self._ensure_agent(request.agent_id)
        object_key = _make_object_key(request.agent_id, request.object_id)
        with self.begin_object_txn(write=False) as txn:
            bytes = txn.get(object_key, default=None)
        
        return grit_store_pb2.LoadResponse(
            agent_id=request.agent_id,
            object_id=request.object_id,
            data=bytes)
    

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
    # Secret Store API
    #=========================================================
    def set_secret(self, request:secret_store_pb2.SetSecretRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        secret_key = _make_secret_key(request.agent_id, request.key)
        with self.begin_secrets_txn() as txn:
            txn.put(secret_key, request.value.encode('utf-8'))
        return None


    def get_secret(self, request:secret_store_pb2.GetSecretRequest)  -> secret_store_pb2.GetSecretResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        secret_key = _make_secret_key(request.agent_id, request.key)
        with self.begin_secrets_txn(write=False) as txn:
            value:bytes = txn.get(secret_key, default=None)
        
        return secret_store_pb2.GetSecretResponse(
            agent_id=request.agent_id,
            key=request.key,
            value=value.decode('utf-8'))

    def get_secrets(self, request:secret_store_pb2.GetSecretsRequest) -> secret_store_pb2.GetSecretsResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        search_key = request.agent_id
        if request.key_prefix is not None:
            search_key = _make_secret_key(request.agent_id, request.key_prefix)
        secrets = {}
        with self.begin_secrets_txn(write=False) as txn:
            cursor = txn.cursor()
            cursor.set_range(search_key)
            while _bytes_startswith(cursor.key(), search_key):
                _, key = _parse_secret_key(cursor.key())
                secrets[key] = cursor.value().decode('utf-8')
                cursor.next()

        return secret_store_pb2.GetSecretsResponse(
            agent_id=request.agent_id,
            secrets=secrets)
    
    def delete_secret(self, request:secret_store_pb2.DeleteSecretRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        self._ensure_agent(request.agent_id)
        secret_key = _make_secret_key(request.agent_id, request.key)
        with self.begin_secrets_txn() as txn:
            txn.delete(secret_key)
        return None
    
    #=========================================================
    # Agent Management API
    #=========================================================  
    def get_agent(self, request:grit_store_pb2.GetAgentRequest) -> grit_store_pb2.GetAgentResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        with self.begin_agents_txn(write=False) as txn:
            agent_id = txn.get(request.agent_did.encode('utf-8'))
            return grit_store_pb2.GetAgentResponse(
                agent_id=agent_id,
                agent_did=request.agent_did)
        
        
    def get_agents(self) -> grit_store_pb2.GetAgentsResponse:
        agents = {}
        with self.begin_agents_txn(write=False) as txn:
            cursor = txn.cursor()
            cursor.first()
            while cursor.next():
                agent_did = cursor.key().decode('utf-8')
                agent_id = cursor.value()
                agents[agent_did] = agent_id
        return grit_store_pb2.GetAgentsResponse(agents=agents)
    

    def create_agent(self, request:grit_store_pb2.CreateAgentRequest) -> grit_store_pb2.CreateAgentResponse:
        if(request is None):
            raise ValueError("request must not be None.")
        

        if(request.agent_did):
            if not request.agent_did.startswith('did:key'):
                #TODO: support other DID methods
                #TODO: more rigorous DID validation
                raise ValueError("agent DID must use the did:key method (see DID spec).")
            if request.agent_did_private_key is None:
                raise ValueError("agent DID private key must be provided, when providing an external DID.")
            agent_did = request.agent_did
            agent_did_private_key = request.agent_did_private_key
            agent_did_private_key_bytes = bytes.fromhex(agent_did_private_key)
            #TODO: check that the private key matches the public key of the DID
        else:
            logger.info("Generating new agent DID.")
            agent_did, _, agent_did_private_key_bytes = generate_did()
            agent_did_private_key = agent_did_private_key_bytes.hex()
            logger.info(f"Generated agent DID: {agent_did}")

        #create the agent in the db
        agents_db = self.get_agents_db()
        object_db = self.get_object_db()
        refs_db = self.get_refs_db()
        with self.env.begin(write=True) as txn:
            #check if the agent already exists
            existing_agent_id = txn.get(agent_did.encode('utf-8'), db=agents_db)
            if existing_agent_id is not None:
                logger.warn(f"Agent for DID '{agent_did}' already exists, will return existing agent.")
                return grit_store_pb2.CreateAgentResponse(
                    agent_id=existing_agent_id,
                    agent_did=agent_did)
            
            logger.info(f"Creating agent for DID '{agent_did}'")
            #since the agent doesn't exist we need to create the root actor for that agent,
            # which determines the agent_id: the agent_id is the actor_id of the root actor
            # the "root actor" is a priviledged actor that represents the runtime and the agent itself
            #
            # creating the root actor is tricky because the db expects the agent_id to already exist,
            # but the the id is not known until all the objects and initial core exists for that root actor
            # however, the trick is to construct all relevant grit objects in memory, derrive the actor id from there
            # and then write all the objects to the db in a single transaction

            import aos.grit.object_serialization as ser
            #TODO: change this to normal "content-type"
            did_blob = Blob({'ct': 's'}, agent_did.encode('utf-8'))
            did_blob_id = ser.get_object_id(ser.blob_to_bytes(did_blob))
            core = {'did': did_blob_id}
            core_id = ser.get_object_id(ser.tree_to_bytes(core))
            agent_id = core_id #the agent id is the core id

            # there is more work to boostrap the agent... we need to create a step
            # implemented as an iterator to make it a bit more readable
            def bootstrap():
                yield did_blob_id, ser.blob_to_bytes(did_blob)
                yield core_id, ser.tree_to_bytes(core)
                #genesis message
                msg = Message(previous=None, headers={"mt": "genesis"}, content=core_id)
                msg_bytes = ser.message_to_bytes(msg)
                msg_id = ser.get_object_id(msg_bytes)
                yield msg_id, msg_bytes
                #genesis step inbox (from agent id to itself, nice old bootstrap!)
                inbox = {agent_id: msg_id}
                inbox_bytes = ser.mailbox_to_bytes(inbox)
                inbox_id = ser.get_object_id(inbox_bytes)
                yield inbox_id, inbox_bytes
                #genesis step
                step = Step(previous=None, actor=agent_id, inbox=inbox_id, outbox=None, core=core_id)
                step_bytes = ser.step_to_bytes(step)
                step_id = ser.get_object_id(step_bytes)
                yield step_id, step_bytes

            #now save the core grit objects
            last_obj_id = None
            for obj_id, obj_bytes in bootstrap():
                txn.put(
                    _make_object_key(agent_id, obj_id), 
                    obj_bytes, 
                    db=object_db)
                last_obj_id = obj_id
            #the last id from the iterator was the step id
            step_id = last_obj_id
            
            #set the initial references (step HEAD)
            txn.put(
                _make_refs_key(agent_id, ref_step_head(agent_id)), 
                step_id, 
                db=refs_db)
            txn.put(
                _make_refs_key(agent_id, ref_root_actor()),
                agent_id,
                db=refs_db)

            #create the agent
            txn.put(agent_did.encode('utf-8'), agent_id, db=agents_db)

            #not thread safe, but should be fine for now (since this is lunning only in one process)
            if self._agents is None:
                self._agents = set()
            self._agents.add(agent_id)

            return grit_store_pb2.CreateAgentResponse(
                agent_id=agent_id,
                agent_did=agent_did)
        

    def delete_agent(self, request:grit_store_pb2.DeleteAgentRequest) -> None:
        if(request is None):
            raise ValueError("request must not be None.")
        
        agents_db = self.get_agents_db()
        object_db = self.get_object_db()
        refs_db = self.get_refs_db()
        secrets_db = self.get_secrets_db()

        with self.env.begin(write=True) as txn:
            #get the agent id
            agent_id = txn.get(request.agent_did.encode('utf-8'), db=agents_db)
            if agent_id is None:
                return None
            
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

            #delete all secrets
            cursor = txn.cursor(db=secrets_db)
            cursor.set_range(agent_id)
            while _bytes_startswith(cursor.key(), agent_id):
                txn.delete(cursor.key())
                cursor.next()

            #delete the agent
            txn.delete(request.agent_did.encode('utf-8'), db=agents_db)
        
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

def _make_secret_key(agent_id:ActorId, secret_key:str) -> bytes:
    return agent_id + secret_key.encode('utf-8')

def _parse_secret_key(key:bytes) -> tuple[ActorId, str]:
    return key[:_GRIT_ID_LEN], key[_GRIT_ID_LEN:].decode('utf-8')

def _bytes_startswith(haystack:bytes, needle:bytes) -> bool:
    return haystack[:len(needle)] == needle

