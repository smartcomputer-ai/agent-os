from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Iterable as _Iterable, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class WorkerRegistrationRequest(_message.Message):
    __slots__ = ("node_id", "manifest")
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    MANIFEST_FIELD_NUMBER: _ClassVar[int]
    node_id: str
    manifest: WorkerManifest
    def __init__(self, node_id: _Optional[str] = ..., manifest: _Optional[_Union[WorkerManifest, _Mapping]] = ...) -> None: ...

class WorkerRegistrationResponse(_message.Message):
    __slots__ = ("ticket",)
    TICKET_FIELD_NUMBER: _ClassVar[int]
    ticket: str
    def __init__(self, ticket: _Optional[str] = ...) -> None: ...

class WorkerManifest(_message.Message):
    __slots__ = ("node_id", "capabilities", "current_agents", "current_actors")
    class CapabilitiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    CAPABILITIES_FIELD_NUMBER: _ClassVar[int]
    CURRENT_AGENTS_FIELD_NUMBER: _ClassVar[int]
    CURRENT_ACTORS_FIELD_NUMBER: _ClassVar[int]
    node_id: str
    capabilities: _containers.ScalarMap[str, str]
    current_agents: _containers.RepeatedCompositeFieldContainer[Agent]
    current_actors: _containers.RepeatedCompositeFieldContainer[Actor]
    def __init__(self, node_id: _Optional[str] = ..., capabilities: _Optional[_Mapping[str, str]] = ..., current_agents: _Optional[_Iterable[_Union[Agent, _Mapping]]] = ..., current_actors: _Optional[_Iterable[_Union[Actor, _Mapping]]] = ...) -> None: ...

class Agent(_message.Message):
    __slots__ = ("agent_id", "agent_did", "grit_address")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    AGENT_DID_FIELD_NUMBER: _ClassVar[int]
    GRIT_ADDRESS_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    agent_did: str
    grit_address: str
    def __init__(self, agent_id: _Optional[bytes] = ..., agent_did: _Optional[str] = ..., grit_address: _Optional[str] = ...) -> None: ...

class Actor(_message.Message):
    __slots__ = ("agent_id", "actor_id")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ...) -> None: ...

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
    __slots__ = ("agent_id", "actor_id", "query_id", "result")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    QUERY_ID_FIELD_NUMBER: _ClassVar[int]
    RESULT_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    query_id: str
    result: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ..., query_id: _Optional[str] = ..., result: _Optional[bytes] = ...) -> None: ...

class ApexToWorker(_message.Message):
    __slots__ = ("type", "messages", "queries", "actors", "agents")
    class ApexToWorkerType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        POKE: _ClassVar[ApexToWorker.ApexToWorkerType]
        GIVE_ACTORS: _ClassVar[ApexToWorker.ApexToWorkerType]
        YANK_ACTORS: _ClassVar[ApexToWorker.ApexToWorkerType]
        ACTOR_MESSAGES: _ClassVar[ApexToWorker.ApexToWorkerType]
        ACTOR_QUERIES: _ClassVar[ApexToWorker.ApexToWorkerType]
    POKE: ApexToWorker.ApexToWorkerType
    GIVE_ACTORS: ApexToWorker.ApexToWorkerType
    YANK_ACTORS: ApexToWorker.ApexToWorkerType
    ACTOR_MESSAGES: ApexToWorker.ApexToWorkerType
    ACTOR_QUERIES: ApexToWorker.ApexToWorkerType
    TYPE_FIELD_NUMBER: _ClassVar[int]
    MESSAGES_FIELD_NUMBER: _ClassVar[int]
    QUERIES_FIELD_NUMBER: _ClassVar[int]
    ACTORS_FIELD_NUMBER: _ClassVar[int]
    AGENTS_FIELD_NUMBER: _ClassVar[int]
    type: ApexToWorker.ApexToWorkerType
    messages: _containers.RepeatedCompositeFieldContainer[ActorMessage]
    queries: _containers.RepeatedCompositeFieldContainer[ActorQuery]
    actors: _containers.RepeatedCompositeFieldContainer[Actor]
    agents: _containers.RepeatedCompositeFieldContainer[Agent]
    def __init__(self, type: _Optional[_Union[ApexToWorker.ApexToWorkerType, str]] = ..., messages: _Optional[_Iterable[_Union[ActorMessage, _Mapping]]] = ..., queries: _Optional[_Iterable[_Union[ActorQuery, _Mapping]]] = ..., actors: _Optional[_Iterable[_Union[Actor, _Mapping]]] = ..., agents: _Optional[_Iterable[_Union[Agent, _Mapping]]] = ...) -> None: ...

class WorkerToApex(_message.Message):
    __slots__ = ("type", "worker_id", "ticket", "messages", "queries")
    class WorkerToApexType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        READY: _ClassVar[WorkerToApex.WorkerToApexType]
        ACTOR_MESSAGES: _ClassVar[WorkerToApex.WorkerToApexType]
        ACTOR_QUERIES: _ClassVar[WorkerToApex.WorkerToApexType]
    READY: WorkerToApex.WorkerToApexType
    ACTOR_MESSAGES: WorkerToApex.WorkerToApexType
    ACTOR_QUERIES: WorkerToApex.WorkerToApexType
    TYPE_FIELD_NUMBER: _ClassVar[int]
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    TICKET_FIELD_NUMBER: _ClassVar[int]
    MESSAGES_FIELD_NUMBER: _ClassVar[int]
    QUERIES_FIELD_NUMBER: _ClassVar[int]
    type: WorkerToApex.WorkerToApexType
    worker_id: str
    ticket: str
    messages: _containers.RepeatedCompositeFieldContainer[ActorMessage]
    queries: _containers.RepeatedCompositeFieldContainer[ActorQueryResult]
    def __init__(self, type: _Optional[_Union[WorkerToApex.WorkerToApexType, str]] = ..., worker_id: _Optional[str] = ..., ticket: _Optional[str] = ..., messages: _Optional[_Iterable[_Union[ActorMessage, _Mapping]]] = ..., queries: _Optional[_Iterable[_Union[ActorQueryResult, _Mapping]]] = ...) -> None: ...
