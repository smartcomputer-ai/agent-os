from __future__ import annotations
from abc import ABC
from dataclasses import dataclass, field
import os
import random
import asyncio
from typing import AsyncIterable
import grpc
import time
from enum import Enum
from async_lru import alru_cache
from aos.grit import *
from aos.wit import *
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc, agent_store_pb2, agent_store_pb2_grpc
from aos.runtime.apex import apex_api_pb2, apex_api_pb2_grpc, apex_workers_pb2, apex_workers_pb2_grpc
from aos.runtime.worker import worker_api_pb2, worker_api_pb2_grpc
from aos.runtime.store.store_client import StoreClient
from aos.runtime.apex.apex_client import ApexClient
from aos.runtime.worker.worker_client import WorkerClient
from aos.runtime.store.agent_object_store import AgentObjectStore
from aos.runtime.store.agent_references import AgentReferences

import logging
logger = logging.getLogger(__name__)

@dataclass
class _AgentState:
    agent_id:AgentId
    object_store: AgentObjectStore
    references: AgentReferences
    worker_id:str|None

class AgentsClient:
    """Gets information about agents, their actors. Allows querying and injecting messages, and subscribing to agents.
    
    Connects to store, apex, and the appropriate worker for each agent.
    """

    def __init__(self, apex_address:str="localhost:50052") -> None:
        self._apex_address = apex_address
        self._apex_client = ApexClient(apex_address)
        self._store_client = None
        self._worker_clients:dict[str, WorkerClient] = {}
        #agent state
        self._agents:dict[AgentId, _AgentState] = {}
        self._agents_lock = asyncio.Lock()

    async def _get_apex_client(self) -> ApexClient:
        return self._apex_client
    
    async def _get_store_client(self) -> StoreClient:
        if self._store_client is not None:
            return self._store_client
        else:
            #it's okay if more than one client gets created, if there is a race condition on first set
            try:
                apex_client = await self._get_apex_client()
                apex_status:apex_api_pb2.GetApexStatusResponse = await apex_client.get_apex_api_stub_async().GetApexStatus(apex_api_pb2.GetApexStatusRequest())
                store_address = apex_status.store_address
                self._store_client = StoreClient(store_address)
                return self._store_client
            except Exception as e:
                logger.error(f"Error getting store client: {e}")
                raise

    async def _get_worker_client(self, worker_id:str, worker_address:str) -> WorkerClient:
        if not worker_id:
            raise ValueError("worker_id must be a non-empty string.")
        if not worker_address:
            raise ValueError("worker_address must be a non-empty string.")
        if worker_id in self._worker_clients:
            return self._worker_clients[worker_id]
        else:
            # rance conditions should not happen here because there is no async call
            worker_client = WorkerClient(worker_address)
            self._worker_clients[worker_id] = worker_client
            return worker_client


    async def _get_agent(self, agent_id:AgentId) -> _AgentState:
        if not is_object_id(agent_id):
            raise ValueError("agent_id must be an ObjectId (bytes).")
        async with self._agents_lock:
            if agent_id in self._agents:
                return self._agents[agent_id]
            else:
                store_client = await self._get_store_client()
                agent = _AgentState(
                    agent_id=agent_id,
                    worker_id=None,
                    object_store=AgentObjectStore(store_client, agent_id),
                    references=AgentReferences(store_client, agent_id))
                self._agents[agent_id] = agent
                return agent

    async def _get_agent_worker_client(self, agent_id:AgentId) -> WorkerClient:
        if not is_object_id(agent_id):
            raise ValueError("agent_id must be an ObjectId (bytes).")
        agent = await self._get_agent(agent_id)
        #check if the worker_id is set for the agent, if not, try to determine it
        if agent.worker_id is None:
            # get the agent map from apex
            # TODO: this is inefficient, create an API that retrieves a single agent
            apex_client = await self._get_apex_client()
            apex_response:apex_api_pb2.GetRunningAgentsResponse = await apex_client.get_apex_api_stub_async().GetRunningAgents(apex_api_pb2.GetRunningAgentsRequest()) 
            running_agent = next((agent for agent in apex_response.agents if agent.agent_id == agent_id), None)
            
            if running_agent is None:
                raise Exception(f"Agent {agent_id.hex()} is not running.")
            if running_agent.worker_id is None:
                raise Exception(f"Agent {agent_id.hex()} is running but has no worker_id.")
            if running_agent.worker_address is None:
                raise Exception(f"Agent {agent_id.hex()} is running but has no worker_address.")
            agent.worker_id = running_agent.worker_id
            return await self._get_worker_client(agent.worker_id, running_agent.worker_address)

        return self._worker_clients[agent.worker_id]

    async def create_agent(self) -> tuple[AgentId, str]:
        store_client = await self._get_store_client()
        agent_response:agent_store_pb2.CreateAgentResponse = await store_client.get_agent_store_stub_async().CreateAgent(agent_store_pb2.CreateAgentRequest())
        return agent_response.agent_id, agent_response.agent_did

    async def get_running_agents(self) -> dict[AgentId, str]:
        apex_client = await self._get_apex_client()
        apex_response:apex_api_pb2.GetRunningAgentsResponse = await apex_client.get_apex_api_stub_async().GetRunningAgents(apex_api_pb2.GetRunningAgentsRequest())
        return {agent.agent_id:agent.agent_did for agent in apex_response.agents}

    async def start_agent(self, agent_id:AgentId):
        apex_client = await self._get_apex_client()
        await apex_client.get_apex_api_stub_async().StartAgent(apex_api_pb2.StartAgentRequest(agent_id=agent_id))

    async def stop_agent(self, agent_id:AgentId):
        apex_client = await self._get_apex_client()
        await apex_client.get_apex_api_stub_async().StopAgent(apex_api_pb2.StopAgentRequest(agent_id=agent_id))

    async def get_object_store(self, agent_id:AgentId) -> ObjectStore:
        agent = await self._get_agent(agent_id)
        return agent.object_store
    
    async def get_references(self, agent_id:AgentId) -> References:
        agent = await self._get_agent(agent_id)
        return agent.references

    async def get_agents(self) -> dict[AgentId, str]:
        """Returns all agents, both running and not running."""
        store_client = await self._get_store_client()
        agents_response:agent_store_pb2.GetAgentsResponse = await store_client.get_agent_store_stub_async().GetAgents(agent_store_pb2.GetAgentsRequest())
        return {agent_id:agent_did for agent_did, agent_id in agents_response.agents.items()}

    @alru_cache(maxsize=1000)  # noqa: B019
    async def agent_exists(self, agent_id:AgentId) -> bool:
        agents = await self.get_agents()
        return agent_id in agents
    
    @alru_cache(maxsize=1000)  # noqa: B019
    async def lookup_agent_by_name(self, agent_name:str) -> AgentId|None:
        agents = await self.get_agents()
        agent_id = next((agent_id for agent_id, name in agents.items() if name == agent_name), None)
        return agent_id
    
    async def get_actors(self, agent_id:AgentId) -> dict[ActorId, str|None]:
        references = await self.get_references(agent_id)
        refs = await references.get_all()
        #keep refs that start with "heads/" to get the actors
        actors_ids = [bytes.fromhex(ref.removeprefix('heads/')) for ref in refs.keys() if ref.startswith("heads/")]
        #get the actor names (the ones that have one)
        named_actors = {ref.removeprefix('actors/'):actor_id for ref,actor_id in refs.items() if ref.startswith('actors/')}
        return {actor_id:named_actors.get(actor_id) for actor_id in actors_ids}

    @alru_cache(maxsize=1000)
    async def lookup_actor_by_name(self, agent_id:AgentId, actor_name:str) -> ActorId|None:
        actors = await self.get_actors(agent_id)
        actor_id = next((actor_id for actor_id, name in actors.items() if name is not None and name == actor_name), None)
        return actor_id
    
    @alru_cache(maxsize=1000)
    async def actor_exists(self, agent_id:AgentId, actor_id) -> bool:
        actors = await self.get_actors(agent_id)
        return actor_id in actors
    
    async def get_object(self, agent_id:AgentId, object_id:ObjectId) -> Object:
        agent = await self._get_agent(agent_id)
        return await agent.object_store.load(object_id)
    

    async def inject_message(self, agent_id:AgentId, message:OutboxMessage) -> MessageId:
        worker_client = await self._get_agent_worker_client(agent_id)
        
        if is_object_id(message.content):
            content_id = message.content
        else:
            #persist the message contents to get a content_id
            object_store = await self.get_object_store(agent_id)
            content_id = await message.content.persist(object_store)

        try:
            inject_response:worker_api_pb2.InjectMessageResponse = await worker_client.get_worker_api_stub_async().InjectMessage(
                worker_api_pb2.InjectMessageRequest(
                    agent_id=agent_id,
                    recipient_id=message.recipient_id,
                    message_data=worker_api_pb2.MessageData(
                        headers=message.headers,
                        is_signal=message.is_signal,
                        previous_id=message.previous_id,
                        content_id=content_id)))
        except grpc.aio.AioRpcError as e:
            if e.code() == grpc.StatusCode.NOT_FOUND:
                async with self._agents_lock:
                    if agent_id in self._agents:
                        self._agents[agent_id].worker_id = None
                raise Exception(f"Agent {agent_id.hex()} not found.")
            else:
                raise e
            
        return inject_response.message_id
        
        
    async def run_query(self, agent_id:AgentId, actor_id:ActorId, query_name:str, query_context:Blob|None) -> ObjectId | Tree | Blob | None:
        worker_client = await self._get_agent_worker_client(agent_id)

        try:
            query_response:worker_api_pb2.RunQueryResponse = await worker_client.get_worker_api_stub_async().RunQuery(
                worker_api_pb2.RunQueryRequest(
                    agent_id=agent_id,
                    actor_id=actor_id,
                    query_name=query_name,
                    query_context_blob=object_to_bytes(query_context) if query_context is not None else None))
        except grpc.aio.AioRpcError as e:
            if e.code() == grpc.StatusCode.NOT_FOUND:
                async with self._agents_lock:
                    if agent_id in self._agents:
                        self._agents[agent_id].worker_id = None
                raise Exception(f"Agent {agent_id.hex()} not found on worker.")
            else:
                raise e
        
        if query_response.HasField("result"):
            if query_response.result is None:
                return None
            elif is_object_id(query_response.result):
                return query_response.result
            else:
                return bytes_to_object(query_response.result)
        return None


    async def subscribe_to_agent(self, agent_id:AgentId) -> AsyncIterable[tuple[ActorId, MessageId, Message]]:
        worker_client = await self._get_agent_worker_client(agent_id)

        response_iterable:AsyncIterable[worker_api_pb2.SubscriptionMessage] = await worker_client.get_worker_api_stub_async().SubscribeToAgent(
            worker_api_pb2.SubscriptionRequest(
                agent_id=agent_id))
        
        try:
            async for response in response_iterable:
                sender_id = response.sender_id
                message_id = response.message_id
                message = Message(
                    headers=response.message_data.headers,
                    previous=response.message_data.previous_id,
                    content=response.message_data.content_id)
                yield sender_id, message_id, message
        except grpc.aio.AioRpcError as e:
            logger.error(f"Error in agent {agent_id.hex()} subscription: {e}")
        finally:
            #when a subscription is terminated by the server, remove the worker id, so we have to check again on the next subscription
            async with self._agents_lock:
                if agent_id in self._agents:
                    self._agents[agent_id].worker_id = None
