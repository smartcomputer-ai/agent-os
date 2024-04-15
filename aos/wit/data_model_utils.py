from grit import *
from . data_model import *

# Functions to load and persist steps, utilizing on the data model classes Inbox, Outbox, and Core
# Mostly used internally in the wit api to make writing wit functions more convenient and ergonomic.

async def load_step(loader:ObjectLoader, actor_id:ActorId, last_step_id:StepId|None, new_inbox:Mailbox|None) -> tuple[Inbox, Outbox, Core]:
    #if there is no last step, then we know this is the genesis step
    if last_step_id is None:
        return await load_step_from_genesis_message(loader, actor_id, new_inbox)
    else:
        return await load_step_from_last(loader, last_step_id, new_inbox)

async def load_step_from_genesis_message(loader:ObjectLoader, actor_id:ActorId, new_inbox:Mailbox) -> tuple[Inbox, Outbox, Core]:
    if(new_inbox is None):
        raise ValueError("new_inbox cannot be None when loading the genesis step.")
    # Find a create message in the inbox:
    # - the create message has the same content id as the actor id since the content of the create message *is* the core of the new actor
    # - it must be the first message sent from an different actor to this actor
    #create a temporary inbox just to find the create message
    tmp_inbox = await load_inbox_from_last(loader, None, new_inbox)
    current_messages = await tmp_inbox.read_new()
    # Serch current_messages for a message with the same content id as this actor id.
    # The create message also must be the first message sent from the sender actor, so it has no previous message.
    create_envelope = next((envelope for envelope in current_messages if envelope.content_id == actor_id and envelope.previous_id is None), None)
    if create_envelope is None:
        #todo: do not throw here, but instead log a warning and return None so that the runtime can try again later
        # the create message might simply have not arrived yet
        # this is probably not needed until there is full, multi-process parallelism in the runtime
        raise ValueError("No create message found in the new_messages list.")
    genesis_core_id = create_envelope.content_id
    genesis_core = await Core.from_core_id(loader, genesis_core_id)
    #create a new inbox that contains only the create message (process other messages in the next step)
    genesis_inbox = await load_inbox_from_last(loader, None, {create_envelope.sender_id: create_envelope.message_id})
    #the outbox will obviously be empty if the actor has never been run
    genesis_outbox = Outbox(Mailbox())
    return (genesis_inbox, genesis_outbox, genesis_core)

async def load_step_from_last(loader:ObjectLoader, last_step_id:StepId, new_inbox:Mailbox|None) -> tuple[Inbox, Outbox, Core]:
    if(last_step_id is None):
        raise ValueError("last_step_id cannot be None.")
    #besides the genesis step, all steps build on the previous step
    last_step:Step = await loader.load(last_step_id)
    if(last_step is None):
        raise ValueError(f"last_step_id '{last_step_id.hex()}' does not exist.")
    #if no new_inbox was proposed (usually by the runtime or executor), then use the inbox from the last step
    if new_inbox is None and last_step.inbox is not None:
        new_inbox = await loader.load(last_step.inbox)
    current_inbox = await load_inbox_from_last(loader, last_step.inbox, new_inbox)
    current_outbox = await load_outbox_from_last(loader, last_step.outbox)
    current_core = await Core.from_core_id(loader, last_step.core)
    return ( current_inbox, current_outbox, current_core)

async def load_inbox_from_last(loader:ObjectLoader, last_inbox:MailboxId|Mailbox|None, new_inbox:Mailbox|None) -> Inbox:
    if last_inbox is None:
        last_inbox = Mailbox()
    if new_inbox is None: #this should be rare, and only happen when no one is communicating with the actor anymore
        new_inbox = Mailbox()
    #create the current inbox
    if(isinstance(last_inbox, MailboxId)):
        return await Inbox.from_inbox_id(loader, last_inbox, new_inbox)
    else:
        return Inbox(loader, last_inbox, new_inbox)

async def load_outbox_from_last(loader:ObjectLoader, last_outbox:MailboxId|Mailbox|None) -> Outbox:
    if last_outbox is None:
        last_outbox = Mailbox()
    if(isinstance(last_outbox, MailboxId)):
        return await Outbox.from_outbox_id(loader, last_outbox)
    else:
        return Outbox(last_outbox)

async def persist_step(object_store:ObjectStore, actor_id:ActorId, last_step_id:StepId|None, inbox:Inbox, outbox:Outbox, core:Core) -> StepId:
    if not inbox.is_empty():
        new_inbox_id = await inbox.persist(object_store)
    else:
        new_inbox_id = None
    if not outbox.is_empty():
        new_outbox_id = await outbox.persist(object_store)
    else:
        new_outbox_id = None
    #todo: validate core again
    new_core_id = await core.persist(object_store)

    new_step = Step(last_step_id, actor_id, new_inbox_id, new_outbox_id, new_core_id)
    new_step_id = await object_store.store(new_step)
    return new_step_id