from __future__ import annotations
from aos.grit import *
from aos.grit import Mailbox
from aos.wit import *
from aos.runtime.core.actor_executor import ExecutionContext, ActorExecutor, _WitExecution, MailboxUpdate
from aos.runtime.core.root_executor import create_or_load_root_actor

async def add_offline_message_to_root_outbox(
        object_store:ObjectStore, 
        references:References, 
        agent_name:str, 
        msg:OutboxMessage, 
        set_previous:bool=False,
        ) -> StepId:
    '''Dangerous! Do not use if the runtime is running!'''
    #extra check
    agent_id, last_step_id = await create_or_load_root_actor(object_store, references, agent_name)
    last_step = await object_store.load(last_step_id)
    if(last_step.outbox is None):
        last_step_outbox = {}
    else:
        last_step_outbox = await object_store.load(last_step.outbox)
    #set previous id if needed
    if(msg.recipient_id in last_step_outbox and set_previous and not msg.is_signal):
        msg.previous_id = last_step_outbox[msg.recipient_id]
    #new outbox
    new_outbox = last_step_outbox.copy()
    msg_id = await msg.persist(object_store)
    new_outbox[msg.recipient_id] = msg_id
    new_outbox_id = await object_store.store(new_outbox)
    #new step
    new_step = Step(last_step_id, agent_id, last_step.inbox, new_outbox_id, last_step.core)
    new_step_id = await object_store.store(new_step)
    await references.set(ref_step_head(agent_id), new_step_id)
    return new_step_id

async def remove_offline_message_from_root_outbox(
        object_store:ObjectStore, 
        references:References, 
        agent_name:str, 
        recipient_id:ActorId,
        ) -> StepId:
    '''Dangerous! Do not use if the runtime is running!'''
    #todo: dedupe the code with add_offline_message_to_agent_outbox
    agent_id, last_step_id = await create_or_load_root_actor(object_store, references, agent_name)
    last_step = await object_store.load(last_step_id)
    if(last_step.outbox is None):
        last_step_outbox = {}
    else:
        last_step_outbox = await object_store.load(last_step.outbox)
    #new outbox
    new_outbox = last_step_outbox.copy()
    if(recipient_id in new_outbox):
        del new_outbox[recipient_id]
    if(len(new_outbox) > 0):
        new_outbox_id = await object_store.store(new_outbox)
    else:
        new_outbox_id = None
    #new step
    new_step = Step(last_step_id, agent_id, last_step.inbox, new_outbox_id, last_step.core)
    new_step_id = await object_store.store(new_step)
    await references.set(ref_step_head(agent_id), new_step_id)
    return new_step_id