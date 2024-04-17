from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Iterable as _Iterable, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class WorkerRegistrationRequest(_message.Message):
    __slots__ = ("worker_id", "manifest")
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    MANIFEST_FIELD_NUMBER: _ClassVar[int]
    worker_id: str
    manifest: WorkerManifest
    def __init__(self, worker_id: _Optional[str] = ..., manifest: _Optional[_Union[WorkerManifest, _Mapping]] = ...) -> None: ...

class WorkerRegistrationResponse(_message.Message):
    __slots__ = ("ticket",)
    TICKET_FIELD_NUMBER: _ClassVar[int]
    ticket: str
    def __init__(self, ticket: _Optional[str] = ...) -> None: ...

class WorkerManifest(_message.Message):
    __slots__ = ("worker_id", "capabilities", "current_actors")
    class CapabilitiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    CAPABILITIES_FIELD_NUMBER: _ClassVar[int]
    CURRENT_ACTORS_FIELD_NUMBER: _ClassVar[int]
    worker_id: str
    capabilities: _containers.ScalarMap[str, str]
    current_actors: _containers.RepeatedCompositeFieldContainer[Actor]
    def __init__(self, worker_id: _Optional[str] = ..., capabilities: _Optional[_Mapping[str, str]] = ..., current_actors: _Optional[_Iterable[_Union[Actor, _Mapping]]] = ...) -> None: ...

class Actor(_message.Message):
    __slots__ = ("agent_id", "actor_id", "grit_address")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    GRIT_ADDRESS_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    grit_address: str
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ..., grit_address: _Optional[str] = ...) -> None: ...

class ActorMessage(_message.Message):
    __slots__ = ("agent_id", "sender_id", "recipient_id", "message_id")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    SENDER_ID_FIELD_NUMBER: _ClassVar[int]
    RECIPIENT_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    sender_id: bytes
    recipient_id: bytes
    message_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., sender_id: _Optional[bytes] = ..., recipient_id: _Optional[bytes] = ..., message_id: _Optional[bytes] = ...) -> None: ...

class ActorQuery(_message.Message):
    __slots__ = ("agent_id", "actor_id", "query_id", "query_name", "context")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    QUERY_ID_FIELD_NUMBER: _ClassVar[int]
    QUERY_NAME_FIELD_NUMBER: _ClassVar[int]
    CONTEXT_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    query_id: str
    query_name: str
    context: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ..., query_id: _Optional[str] = ..., query_name: _Optional[str] = ..., context: _Optional[bytes] = ...) -> None: ...

class ActorQueryResult(_message.Message):
    __slots__ = ("agent_id", "actor_id", "query_id", "tree_id", "blob")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    QUERY_ID_FIELD_NUMBER: _ClassVar[int]
    TREE_ID_FIELD_NUMBER: _ClassVar[int]
    BLOB_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    query_id: str
    tree_id: bytes
    blob: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ..., query_id: _Optional[str] = ..., tree_id: _Optional[bytes] = ..., blob: _Optional[bytes] = ...) -> None: ...

class ApexToWorkerMessage(_message.Message):
    __slots__ = ("type", "message", "query", "actor")
    class MessageType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        PING: _ClassVar[ApexToWorkerMessage.MessageType]
        GIVE_ACTOR: _ClassVar[ApexToWorkerMessage.MessageType]
        YANK_ACTOR: _ClassVar[ApexToWorkerMessage.MessageType]
        ACTOR_MESSAGE: _ClassVar[ApexToWorkerMessage.MessageType]
        ACTOR_QUERIE: _ClassVar[ApexToWorkerMessage.MessageType]
    PING: ApexToWorkerMessage.MessageType
    GIVE_ACTOR: ApexToWorkerMessage.MessageType
    YANK_ACTOR: ApexToWorkerMessage.MessageType
    ACTOR_MESSAGE: ApexToWorkerMessage.MessageType
    ACTOR_QUERIE: ApexToWorkerMessage.MessageType
    TYPE_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    QUERY_FIELD_NUMBER: _ClassVar[int]
    ACTOR_FIELD_NUMBER: _ClassVar[int]
    type: ApexToWorkerMessage.MessageType
    message: ActorMessage
    query: ActorQuery
    actor: Actor
    def __init__(self, type: _Optional[_Union[ApexToWorkerMessage.MessageType, str]] = ..., message: _Optional[_Union[ActorMessage, _Mapping]] = ..., query: _Optional[_Union[ActorQuery, _Mapping]] = ..., actor: _Optional[_Union[Actor, _Mapping]] = ...) -> None: ...

class WorkerToApexMessage(_message.Message):
    __slots__ = ("type", "worker_id", "ticket", "message", "query")
    class MessageType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        PING: _ClassVar[WorkerToApexMessage.MessageType]
        READY: _ClassVar[WorkerToApexMessage.MessageType]
        ACTOR_MESSAGE: _ClassVar[WorkerToApexMessage.MessageType]
        ACTOR_QUERIE: _ClassVar[WorkerToApexMessage.MessageType]
    PING: WorkerToApexMessage.MessageType
    READY: WorkerToApexMessage.MessageType
    ACTOR_MESSAGE: WorkerToApexMessage.MessageType
    ACTOR_QUERIE: WorkerToApexMessage.MessageType
    TYPE_FIELD_NUMBER: _ClassVar[int]
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    TICKET_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    QUERY_FIELD_NUMBER: _ClassVar[int]
    type: WorkerToApexMessage.MessageType
    worker_id: str
    ticket: str
    message: ActorMessage
    query: ActorQuery
    def __init__(self, type: _Optional[_Union[WorkerToApexMessage.MessageType, str]] = ..., worker_id: _Optional[str] = ..., ticket: _Optional[str] = ..., message: _Optional[_Union[ActorMessage, _Mapping]] = ..., query: _Optional[_Union[ActorQuery, _Mapping]] = ...) -> None: ...
