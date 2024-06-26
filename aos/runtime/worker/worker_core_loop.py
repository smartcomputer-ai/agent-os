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
from aos.runtime.core import *
from aos.runtime.store import grit_store_pb2, grit_store_pb2_grpc
from aos.runtime.store import agent_store_pb2, agent_store_pb2_grpc
from aos.runtime.apex import apex_api_pb2, apex_workers_pb2, apex_workers_pb2_grpc
from aos.runtime.apex.apex_client import ApexClient
from aos.runtime.worker import worker_api_pb2, worker_api_pb2_grpc
from aos.runtime.store.store_client import StoreClient
from aos.runtime.store.agent_object_store import AgentObjectStore
from aos.runtime.store.agent_references import AgentReferences

import logging
logger = logging.getLogger(__name__)

#==============================================================
# Documentation
#==============================================================


#==============================================================
# Type defs and constants
#==============================================================
AgentId = ActorId #the agent is defined by the id of the root actor, so technically, it's an actor id too
WorkerId = str
MailboxUpdate = tuple[ActorId, ActorId, MessageId] # sender_id, recipient_id, message_id


#==============================================================
# Worker State Management Classes
#==============================================================

@dataclass(slots=True)
class WorkerCoreState:
    capabilities: dict[str, str] = field(default_factory=dict)
    store_clients: dict[str, StoreClient] = field(default_factory=dict)
    assigned_agents: dict[AgentId, apex_workers_pb2.Agent] = field(default_factory=dict)
    runtimes: dict[AgentId, Runtime] = field(default_factory=dict)
    runtime_tasks: dict[AgentId, asyncio.Task] = field(default_factory=dict)
    runtime_subscriptions: dict[AgentId, list[asyncio.Task]] = field(default_factory=dict)
    

#==============================================================
# Util Classes (for events below)
#==============================================================
class _EventWithResult(ABC):
    def __init__(self) -> None:
        self._result_event = asyncio.Event()
        self._result = None

    async def wait_for_result(self, timeout_seconds:float=90):
        """Wait for the callback to be called. If the callback is not called within the timeout, a asyncio.TimeoutError is raised."""
        await asyncio.wait_for(self._result_event.wait(), timeout_seconds)
        return self._result
    
    def set_result(self, result):
        self._result = result
        self._result_event.set()

class _EventWithCompletion(ABC):
    def __init__(self) -> None:
        self._completion_event = asyncio.Event()

    async def wait_for_completion(self, timeout_seconds:float=90):
        """Wait for the callback to be called. If the callback is not called within the timeout, a asyncio.TimeoutError is raised."""
        await asyncio.wait_for(self._completion_event.wait(), timeout_seconds)
    
    def set_completion(self):
        self._completion_event.set()


#==============================================================
# Worker Core Loop Class
# Implmented as a single, asynchronous loop
# all interactions with the loop are through events
#==============================================================
class WorkerCoreLoop:

    @dataclass(frozen=True, slots=True)
    class _GiveAgentEvent:
        assignment:apex_workers_pb2.AgentAssignment

    @dataclass(frozen=True, slots=True)
    class _YankAgentEvent:
        assignment:apex_workers_pb2.AgentAssignment

    class _InjectMessageEvent(_EventWithResult):
        agent_id:AgentId
        inject_request:worker_api_pb2.InjectMessageRequest
        def __init__(self, inject_request:worker_api_pb2.InjectMessageRequest) -> None:
            super().__init__()
            self.agent_id = inject_request.agent_id
            self.inject_request = inject_request

        async def wait_for_result(self, timeout_seconds:float=90) -> worker_api_pb2.InjectMessageResponse:
            return await super().wait_for_result(timeout_seconds)

    class _RunQueryEvent(_EventWithResult):
        agent_id:AgentId
        query_request:worker_api_pb2.RunQueryRequest
        def __init__(self, query_request:worker_api_pb2.RunQueryRequest) -> None:
            super().__init__()
            self.agent_id = query_request.agent_id
            self.query_request = query_request

        async def wait_for_result(self, timeout_seconds:float=90) -> worker_api_pb2.RunQueryResponse:
            return await super().wait_for_result(timeout_seconds)
        
    @dataclass(frozen=True, slots=True)
    class _SubscribeToAgentEvent:
        agent_id:AgentId
        subscription_request:worker_api_pb2.SubscriptionRequest
        to_subscription_queue:asyncio.Queue[worker_api_pb2.SubscriptionMessage]


    def __init__(
            self,
            store_address:str,
            apex_address:str,
            worker_address:str,
            worker_id:str,
            external_storage_dir:str|None=None,
            all_agents_subscription_queue:asyncio.Queue[worker_api_pb2.SubscriptionMessage]|None=None, #sends all root actor messages to this queue for all hosted agents
            presence:Presence|None=None, #a service for actors to check presence of a user and publish on a channel
            ) -> None:
        
        self._store_address = store_address
        self._apex_address = apex_address
        self._worker_address = worker_address
        self._worker_id = worker_id
        self._external_storage_dir = external_storage_dir
        self._cancel_event = asyncio.Event()
        self._running_event = asyncio.Event()
        self._event_queue = asyncio.Queue()
        self._state_copy = None
        self._state_copy_lock = asyncio.Lock()
        self._all_agents_subscription_queue = all_agents_subscription_queue
        self._presence = presence

    #==============================================================
    # Main Loop
    #==============================================================
    # async def _make_state_copy(self, loop_state:WorkerCoreState):
    #     async with self._state_copy_lock:
    #         self._state_copy = loop_state.deep_copy()

    # async def get_state_copy(self) -> WorkerCoreState:
    #     async with self._state_copy_lock:
    #         return self._state_copy
        
    async def wait_until_running(self):
        await self._running_event.wait()

    def stop(self):
        self._cancel_event.set()
        
        
    async def start(self):
        logger.info(f"Starting worker core loop: {self._worker_id}")

        #needed for re-connects
        previously_assigned_agents:list[AgentId]|None = None

        #outer loop of worker is trying to maintain a connection to the apex server
        await asyncio.sleep(0) #yield to allow other tasks to run
        self._running_event.set() #signal that apex is about to start
        two_way_task = None
        backoff = 1
        while True:
            #todo: but connect into loop
            to_apex_queue:asyncio.Queue[apex_workers_pb2.WorkerToApexMessage] = asyncio.Queue()
            apex_client = ApexClient(self._apex_address)
            #TODO: maintain connection to store client here too (but maybe only do this in the store refactor (when agent-worker info comes from the store))

            try:
                logger.info(f"Opening gRPC channel to apex...")
                await apex_client.wait_for_async_channel_ready(timeout_seconds=30*60) #30 minutes
                worker_stub = apex_client.get_apex_workers_stub_async()

                #register worker
                register_response = await worker_stub.RegisterWorker(apex_workers_pb2.WorkerRegistrationRequest(
                    worker_id=self._worker_id,
                    worker_address=self._worker_address))
                #the ticket is needed to connect to the apex-worker duplex stream
                ticket = register_response.ticket
                logger.info(f"Registered worker. Ticket: {ticket}")

                #setup the two-way stream
                #needs to run as a task so we can run the main worker loop further down
                two_way_task = asyncio.create_task(self._setup_two_way_stream(worker_stub, to_apex_queue))
                
                logger.info(f"Connected worker to apex, starting worker loop.")
                previously_assigned_agents = await self._worker_loop(
                    ticket,
                    to_apex_queue,
                    previously_assigned_agents)
                
                backoff = 1
            except grpc.aio.AioRpcError as e:
                logger.warning(f"Connection to apex failed: {e.code()}: {e.details()}")
            except Exception as e:
                logger.error(f"Error in worker loop: {e}")
                raise
            finally:
                if two_way_task is not None:
                    two_way_task.cancel()
                await apex_client.close()
                logger.info(f"Closed gRPC channel to apex.")
                await asyncio.sleep(backoff)
                backoff = min(backoff * 1.5, 60) #max 1 minute backoff
    

    async def _setup_two_way_stream(
            self,
            worker_stub:apex_workers_pb2_grpc.ApexWorkersStub,
            to_apex_queue:asyncio.Queue[apex_workers_pb2.WorkerToApexMessage],
            ):
        
        #convert the queue to an iterator (which is what the gRPC api expects)
        # cancel the queue/iterator by adding a None item 
        async def _queue_to_async_iterator(queue:asyncio.Queue) -> AsyncIterator:
            while True:
                event = await queue.get()
                if event is None:
                    break
                yield event

        #connect to the duplex stream
        #TODO: does this need an "await"?
        to_worker_iterator:AsyncIterator[apex_workers_pb2.ApexToWorkerMessage] = worker_stub.ConnectWorker(
                    _queue_to_async_iterator(to_apex_queue))

        try:
            async for message in to_worker_iterator:
                if message.type == apex_workers_pb2.ApexToWorkerMessage.PING:
                    logger.info(f"Received ping from apex")
                elif message.type == apex_workers_pb2.ApexToWorkerMessage.GIVE_AGENT:
                    await self._event_queue.put(self._GiveAgentEvent(message.assignment))
                elif message.type == apex_workers_pb2.ApexToWorkerMessage.YANK_AGENT:
                    await self._event_queue.put(self._YankAgentEvent(message.assignment))
        except grpc.aio.AioRpcError as e:
            logger.warning(f"Connection to apex was closed: {e.code()}: {e.details()}")
        except Exception as e:
            logger.error(f"Error in worker loop: {e}")
            raise
        finally:
            #how to do cleanup?
            logger.info(f"Closing worker-to-apex and main event queue.")
            to_apex_queue.put_nowait(None)
            self._event_queue.put_nowait(None) #stops the main queue


    async def _worker_loop(
        self,
        ticket:str,
        to_apex_queue:asyncio.Queue[apex_workers_pb2.WorkerToApexMessage],
        previously_assigned_agents:list[AgentId]|None = None,
        ):
    
        logger.info(f"Starting inner worker core loop")

        worker_state = WorkerCoreState()

        #connect with apex by sending a READY message
        await to_apex_queue.put(apex_workers_pb2.WorkerToApexMessage(
            type=apex_workers_pb2.WorkerToApexMessage.READY,
            worker_id=self._worker_id,
            ticket=ticket,
            manifest=apex_workers_pb2.WorkerManifest(
                worker_id=self._worker_id,
                capabilities=worker_state.capabilities,
                desired_agents=previously_assigned_agents)))

        while not self._cancel_event.is_set():
            try:
                event = await asyncio.wait_for(self._event_queue.get(), 0.05)
            except asyncio.TimeoutError:
                continue #test for cancel (in the while condition) and try again
            
            if event is None:
                break #go to cleanup

            if isinstance(event, self._GiveAgentEvent):
                await self._handle_give_agent(event, worker_state)

            elif isinstance(event, self._YankAgentEvent):
                await self._handle_yank_agent(event, worker_state)

            elif isinstance(event, self._InjectMessageEvent):
                await self._handle_message_injection(event, worker_state)

            elif isinstance(event, self._RunQueryEvent):
                await self._handle_query(event, worker_state)

            elif isinstance(event, self._SubscribeToAgentEvent):
                await self._handle_subscribe_to_agent(event, worker_state)

        logger.info(f"Cleaning up inner worker state...")
        await to_apex_queue.put(None)

        for runtime in worker_state.runtimes.values():
            runtime.stop()
        runtime_tasks = list(worker_state.runtime_tasks.values())
        if len(runtime_tasks) > 0:
            await asyncio.wait(runtime_tasks, timeout=1.0)

        for store_client in worker_state.store_clients.values():
            await store_client.close(grace_period=1.0)
        
        #so the worker can request them in the next connect loop again
        return list(worker_state.assigned_agents.keys())


    #==============================================================
    # Event Handlers
    #==============================================================
    async def _handle_give_agent(self, event:_GiveAgentEvent, worker_state:WorkerCoreState):
        agent_id:AgentId = event.assignment.agent_id
        agent = event.assignment.agent
        #check if worker already runs the agent
        if agent_id in worker_state.assigned_agents:
            logger.warning(f"Received agent {agent_id.hex()}, but it is already running on this worker. NO-OP.")
            return
        logger.info(f"Received agent {agent_id.hex()} ({agent.point}), will run it...")

        worker_state.assigned_agents[agent_id] = agent
        #create a store client for the agent
        store_address = self._store_address 
        #TODO: move worker_state.store_clients to outer loop
        if store_address not in worker_state.store_clients:
            store_client = StoreClient(store_address)
            await store_client.wait_for_async_channel_ready()
            worker_state.store_clients[store_address] = store_client
        else:
            store_client = worker_state.store_clients[store_address]

        #stores
        object_store = AgentObjectStore(store_client, agent_id)
        references = AgentReferences(store_client, agent_id)

        #create a runtime for the agent
        runtime = Runtime(
            store=object_store, 
            references=references, 
            point=agent.point,
            external_storage_dir=self._external_storage_dir,
            presence=self._presence,)
        worker_state.runtimes[agent_id] = runtime

        async def runtime_runner(agent_id:AgentId, runtime:Runtime):
            try:
                await runtime.start()
            except Exception as e:
                logger.error(f"Error in runtime for {agent_id.hex()}.", exc_info=e)
                #TODO: retry and/or return to apex
                raise

        runtime_task = asyncio.create_task(runtime_runner(agent_id, runtime))
        if self._all_agents_subscription_queue is not None:
            await runtime.wait_until_running()
            logger.info(f"Subscribing the 'all subscriptions queue' to agent {agent_id.hex()} runtime...")
            #add an event that subscribes to this agents runtime with the all_agents_subscription_queue
            await self._event_queue.put(
                self._SubscribeToAgentEvent(
                    agent_id=agent_id,
                    subscription_request=worker_api_pb2.SubscriptionRequest(agent_id=agent_id),
                    to_subscription_queue=self._all_agents_subscription_queue))
            
        worker_state.runtime_tasks[agent_id] = runtime_task
        logger.info(f"Started runtime for {agent_id.hex()} ({agent.point}).")


    async def _handle_yank_agent(self, event:_YankAgentEvent, worker_state:WorkerCoreState):
        agent_id = event.assignment.agent_id
        if agent_id not in worker_state.assigned_agents:
            logger.error(f"Tried to yank agent {agent_id.hex()}, but it is currently not running on this worker.")
            return
        logger.info(f"Yanking agent {agent_id.hex()}, will stop it...")

        runtime = worker_state.runtimes[agent_id]
        runtime_task = worker_state.runtime_tasks[agent_id]
        runtime.stop()
        try:
            await asyncio.wait_for(runtime_task, 2.0)
        except asyncio.TimeoutError as e:
            raise Exception(f"timeout while stopping runtime for agent {agent_id.hex()}.") from e

        del worker_state.runtime_tasks[agent_id]
        del worker_state.runtimes[agent_id]
        del worker_state.assigned_agents[agent_id]
        logger.info(f"Stopped runtime for {agent_id.hex()}.")


    async def _handle_message_injection(self, event:_InjectMessageEvent, worker_state:WorkerCoreState):
        agent_id = event.agent_id
        if agent_id not in worker_state.assigned_agents:
            logger.error(f"Tried to inject message into agent {agent_id.hex()}, but it is currently not running on this worker.")
            event.set_result(None)
            return
        
        injection = event.inject_request
        runtime = worker_state.runtimes[agent_id]
        if injection.message_id:
            #if the message_id was set, it is a mailbox update
            message_id = await runtime.inject_mailbox_update((injection.agent_id, injection.recipient_id, injection.message_id))
        else:
            #otherwise, create a new message via mailbox update
            msg = OutboxMessage(injection.recipient_id, injection.message_data.is_signal)
            if injection.message_data.headers is not None and len(injection.message_data.headers) > 0:
                msg.headers = injection.message_data.headers
            if injection.message_data.content_id is not None:
                msg.content = injection.message_data.content_id
            elif injection.message_data.content_blob is not None:
                #the data is a serialized Grit blob (not just bytes), so it needs to be persisted in Grit first to get the content_id
                content_id = await runtime.ctx.store.store(injection.message_data.content_blob)
                msg.content = content_id
            else:
                raise ValueError("Message data content is empty, set either content_id or content_blob.")
            message_id = await runtime.inject_message(msg)
        event.set_result(worker_api_pb2.InjectMessageResponse(
            agent_id=agent_id,
            message_id=message_id))
        
        logger.info(f"Injected message into {agent_id.hex()} (message_id: {message_id.hex()}).")


    async def _handle_query(self, event:_RunQueryEvent, worker_state:WorkerCoreState):
        agent_id = event.agent_id
        if agent_id not in worker_state.assigned_agents:
            logger.error(f"Tried to query agent {agent_id.hex()}, but agent is currently not running on this worker.")
            event.set_result(None)
            return
        
        query = event.query_request
        if query.HasField("context"):
            #deserialize to a Grit blob not raw binary bytes
            contect_blob = bytes_to_object(query.context)
        else:
            contect_blob = None
        runtime = worker_state.runtimes[agent_id]
        try:
            result = await runtime.query_executor.run(
                query.actor_id,
                query.query_name,
                contect_blob,
            )
        except Exception as e:
            logger.error(f"Error while running query ({query.query_name}) for {agent_id.hex()}.", exc_info=e)
            event.set_result(Exception(f"Error while running query ({query.query_name}) for {agent_id.hex()}: {str(e)}"))
            return

        final_result = None
        if result is None:
            pass
        elif is_object_id(result):
            final_result = result
        elif is_tree(result) or is_blob(result):
            final_result = object_to_bytes(result)
        else:
            event.set_result(ValueError(f"Error while running query ({query.query_name}) for for {agent_id.hex()}: result is not a valid type (must be object id, tree, or blob), was: {type(result)}."))
            return
        
        response = worker_api_pb2.RunQueryResponse(
            agent_id=agent_id,
            actor_id=query.actor_id,
            result=final_result)
        event.set_result(response)


    async def _handle_subscribe_to_agent(self, event:_SubscribeToAgentEvent, worker_state:WorkerCoreState):
        agent_id = event.agent_id
        if agent_id not in worker_state.assigned_agents:
            logger.error(f"Tried to subscribe to agent {agent_id.hex()}, but agent is currently not running on this worker.")
            event.to_subscription_queue.put_nowait(None)
            return

        runtime = worker_state.runtimes[agent_id]
        if worker_state.runtime_subscriptions.get(agent_id) is None:
            worker_state.runtime_subscriptions[agent_id] = []
        subscription_task = asyncio.create_task(self._connect_agent_subscription(runtime, event.to_subscription_queue))
        worker_state.runtime_subscriptions[agent_id].append(subscription_task)
        logger.info(f"Subscription to agent {agent_id.hex()} runtime created.")


    async def _connect_agent_subscription(self, runtime:Runtime, queue:asyncio.Queue[worker_api_pb2.SubscriptionMessage]):
        try:
            with runtime.subscribe_to_messages() as subscription_queue:
                while True:
                    mailbox_update:MailboxUpdate = await subscription_queue.get()
                    if mailbox_update is None:
                        break
                    sender_id = mailbox_update[0]
                    recipient_id = mailbox_update[1] #the recipient is the root actor which is the same as the agent id
                    message_id = mailbox_update[2]
                    #load the message contents to provide more info to the subscriber
                    message:Message = await runtime.ctx.store.load(message_id)
                    subscription_message = worker_api_pb2.SubscriptionMessage(
                        agent_id=runtime.agent_id,
                        sender_id=sender_id,
                        message_id=message_id,
                        message_data=worker_api_pb2.MessageData(
                            headers=message.headers,
                            previous_id=message.previous,
                            is_signal=(message.previous is None),
                            content_id=message.content,
                        ))
                    await queue.put(subscription_message)

        except asyncio.CancelledError as ce:
            logger.info(f"Subscription to agent {runtime.agent_id.hex()} was cancelled.")
        except Exception as e:
            logger.error(f"Error while subscribing to agent {runtime.agent_id.hex()}.", exc_info=e)
        finally:
            queue.put_nowait(None)

    #==============================================================
    # Worker interaction APIs 
    # Works by injecting events into the main loop
    #==============================================================
    def _ensure_running(self):
        if not self._running_event.is_set() or self._cancel_event.is_set():
            raise RuntimeError("Worker core loop is not running.")
 

    async def inject_message(self, inject_request:worker_api_pb2.InjectMessageRequest) -> worker_api_pb2.InjectMessageResponse:
        self._ensure_running()
        event = self._InjectMessageEvent(inject_request)
        await self._event_queue.put(event)
        return await event.wait_for_result(timeout_seconds=5)


    async def run_query(self, query_request:worker_api_pb2.RunQueryRequest) -> worker_api_pb2.RunQueryResponse:
        self._ensure_running()
        event = self._RunQueryEvent(query_request)
        await self._event_queue.put(event)
        return await event.wait_for_result(timeout_seconds=5)


    async def subscribe_to_agent(self, subscription_request:worker_api_pb2.SubscriptionRequest) -> asyncio.Queue[worker_api_pb2.SubscriptionMessage]:
        self._ensure_running()
        agent_id = subscription_request.agent_id
        to_subscription_queue = asyncio.Queue()
        event = self._SubscribeToAgentEvent(
            agent_id=agent_id, 
            subscription_request=subscription_request,
            to_subscription_queue=to_subscription_queue)
        await self._event_queue.put(event)
        return to_subscription_queue



