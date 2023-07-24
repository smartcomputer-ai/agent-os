from typing import NamedTuple

# Type aliases and structures that define the entire object model for the Grit object store.

ObjectId = bytes #32 bytes, sha256 of bytes of object

BlobId = ObjectId
Headers = dict[str, str]
Blob = NamedTuple("Blob",
    [('headers', Headers | None),
     ('data', bytes)])

TreeId = ObjectId
Tree = dict[str, BlobId | TreeId] # a tree key must be an ascii string

ActorId = ObjectId # hash of core of message that created the actor, i.e, object id of the core tree
MessageId = ObjectId
Message = NamedTuple("Message", 
    [('previous', MessageId | None), #if none, it's a signal, otherwise, a queue
     ('headers', Headers | None),
     ('content', BlobId | TreeId)])
MailboxId = ObjectId
Mailbox = dict[ActorId, MessageId]

StepId = ObjectId
Step = NamedTuple("Step",
    [('previous', StepId | None),
     ('actor', ActorId),
     ('inbox', MailboxId | None),
     ('outbox', MailboxId | None),
     ('core', TreeId)])

Object = Blob | Tree | Message | Mailbox | Step



