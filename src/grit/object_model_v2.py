from typing import NamedTuple

# Type aliases and structures that define the entire object model for the Grit object store.

ObjectId = bytes #32 bytes, sha256 of bytes of object

BlobId = ObjectId
TreeId = ObjectId
ListId = ObjectId

Headers = dict[str, str]
Blob = NamedTuple("Blob",
    [('headers', Headers | None),
     ('data', bytes)])

Tree = dict[str, BlobId | TreeId | ListId] # a tree key must be an ascii string

List = list[BlobId | TreeId | ListId] #NEW

ActorId = ObjectId # hash of core of message that created the actor, i.e, object id of the core tree
MessageId = ObjectId
Message = NamedTuple("Message", 
    [('previous', MessageId | None), #if none, it's a signal, otherwise, a queue
     ('headers', Headers | None),
     #('type', str), #NEW aka, "message_type"/"mt" -- is this a good idea, or should it remain part of the headers? 
                     # the pro is that the message types could be made more explicit in the object model here since the runtime inspects the message types substiantly (e.g., "genesis", "update", and, in the future "gc/garbage/disconnect")
     ('content', BlobId | TreeId | ListId | None)]) #NEW with None option, because many messages are just a singal or a ping, and have no content
MailboxId = ObjectId
Mailbox = dict[ActorId, MessageId]

StepId = ObjectId
Step = NamedTuple("Step",
    [('previous', StepId | None),
     ('actor', ActorId),
     ('inbox', MailboxId | None),
     ('outbox', MailboxId | None),
     ('core', TreeId)]) #still, cores must be trees and not a list (unlike JSON, where the top level can be a list or a dict)

Object = Blob | Tree | List | Message | Mailbox | Step



