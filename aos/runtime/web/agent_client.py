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

@dataclass(frozen=True)
class _AgentConnection:
    store_client: StoreClient
    worker_client: WorkerClient
    object_store: AgentObjectStore
    references: AgentReferences

class AgentClient:
    
    #todo: use async locks
    
    def __init__(self, apex_address:str="localhost:50052") -> None:
        self._apex_address = apex_address
        self._apex_client = ApexClient(apex_address)
        self._store_client = None
        self._worker_clients = {}
        self._agent_connections = {}
        self._lock = asyncio.Lock()

    async def _get_apex_client(self) -> ApexClient:
        try:
            await self._apex_client.wait_for_async_channel_ready(timeout_seconds=2)
        except asyncio.TimeoutError:
            logger.error(f"Timeout waiting for apex client to be ready.")
            raise
        return self._apex_client
    

    async def _get_store_client(self) -> StoreClient:
        async with self._lock:
            if self._store_client is None:
                store_client = None
            else:
                store_client = self._store_client

        if store_client is None:
            try:
                apex_client = await self._get_apex_client()
                apex_status = await apex_client.get_apex_api_stub_async().GetApexStatus(apex_api_pb2.GetApexStatusRequest())
                store_address = apex_status.store_address
                store_client = StoreClient(store_address)
                async with self._lock:
                    self._store_client = store_client
            except Exception as e:
                logger.error(f"Error getting store client: {e}")
                raise

        try:
            await store_client.wait_for_async_channel_ready(timeout_seconds=2)
        except asyncio.TimeoutError:
            logger.error(f"Timeout waiting for store client to be ready.")
            raise
        return store_client

    async def _get_worker_client(self, worker_id:str, worker_address:str) -> WorkerClient:
        if worker_id not in self._worker_clients:
            worker_client = WorkerClient(worker_address)
            self._worker_clients[worker_id] = worker_client
        else:
            worker_client = self._worker_clients[worker_id]

        try:
            await worker_client.wait_for_async_channel_ready(timeout_seconds=2)
        except asyncio.TimeoutError:
            logger.error(f"Timeout waiting for worker client {worker_id} to be ready.")
            raise

        return worker_client

    async def _get_agent_connection(self, agent_id:AgentId) -> _AgentConnection:
        async with self._lock:
            if agent_id in self._agent_connections:
                return self._agent_connections[agent_id]

        # try to create the agent connection
        # get the agent map from apex
        apex_client = await self._get_apex_client()
        apex_response:apex_api_pb2.GetRunningAgentsResponse = apex_client.get_apex_api_stub_async().GetRunningAgents(apex_api_pb2.GetRunningAgentsRequest()) 
        running_agent = next((agent for agent in apex_response.agents if agent.agent_id == agent_id), None)
        if running_agent is None:
            raise Exception(f"Agent {agent_id.hex()} not found in running agents from apex.")
        
        # get the store client
        store_client = await self._get_store_client()

        # get the worker client
        worker_client = await self._get_worker_client(running_agent.worker_id, running_agent.worker_address)

        # create the agent object store
        object_store = AgentObjectStore(store_client, agent_id)
        references = AgentReferences(store_client, agent_id)

        agent_connection = _AgentConnection(store_client, worker_client, object_store, references)

        async with self._lock:
            self._agent_connections[agent_id] = agent_connection
            
        return agent_connection



