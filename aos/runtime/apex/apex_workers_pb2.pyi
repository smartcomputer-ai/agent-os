from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Iterable as _Iterable, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class WorkerRegistrationRequest(_message.Message):
    __slots__ = ("worker_id",)
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    worker_id: str
    def __init__(self, worker_id: _Optional[str] = ...) -> None: ...

class WorkerRegistrationResponse(_message.Message):
    __slots__ = ("ticket",)
    TICKET_FIELD_NUMBER: _ClassVar[int]
    ticket: str
    def __init__(self, ticket: _Optional[str] = ...) -> None: ...

class WorkerManifest(_message.Message):
    __slots__ = ("worker_id", "capabilities", "current_agents", "desired_agents")
    class CapabilitiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    CAPABILITIES_FIELD_NUMBER: _ClassVar[int]
    CURRENT_AGENTS_FIELD_NUMBER: _ClassVar[int]
    DESIRED_AGENTS_FIELD_NUMBER: _ClassVar[int]
    worker_id: str
    capabilities: _containers.ScalarMap[str, str]
    current_agents: _containers.RepeatedCompositeFieldContainer[Agent]
    desired_agents: _containers.RepeatedScalarFieldContainer[bytes]
    def __init__(self, worker_id: _Optional[str] = ..., capabilities: _Optional[_Mapping[str, str]] = ..., current_agents: _Optional[_Iterable[_Union[Agent, _Mapping]]] = ..., desired_agents: _Optional[_Iterable[bytes]] = ...) -> None: ...

class Agent(_message.Message):
    __slots__ = ("agent_id", "agent_did", "store_address", "capabilities")
    class CapabilitiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    AGENT_DID_FIELD_NUMBER: _ClassVar[int]
    STORE_ADDRESS_FIELD_NUMBER: _ClassVar[int]
    CAPABILITIES_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    agent_did: str
    store_address: str
    capabilities: _containers.ScalarMap[str, str]
    def __init__(self, agent_id: _Optional[bytes] = ..., agent_did: _Optional[str] = ..., store_address: _Optional[str] = ..., capabilities: _Optional[_Mapping[str, str]] = ...) -> None: ...

class AgentAssignment(_message.Message):
    __slots__ = ("agent_id", "agent")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    AGENT_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    agent: Agent
    def __init__(self, agent_id: _Optional[bytes] = ..., agent: _Optional[_Union[Agent, _Mapping]] = ...) -> None: ...

class ApexToWorkerMessage(_message.Message):
    __slots__ = ("type", "assignment")
    class MessageType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        PING: _ClassVar[ApexToWorkerMessage.MessageType]
        GIVE_AGENT: _ClassVar[ApexToWorkerMessage.MessageType]
        YANK_AGENT: _ClassVar[ApexToWorkerMessage.MessageType]
    PING: ApexToWorkerMessage.MessageType
    GIVE_AGENT: ApexToWorkerMessage.MessageType
    YANK_AGENT: ApexToWorkerMessage.MessageType
    TYPE_FIELD_NUMBER: _ClassVar[int]
    ASSIGNMENT_FIELD_NUMBER: _ClassVar[int]
    type: ApexToWorkerMessage.MessageType
    assignment: AgentAssignment
    def __init__(self, type: _Optional[_Union[ApexToWorkerMessage.MessageType, str]] = ..., assignment: _Optional[_Union[AgentAssignment, _Mapping]] = ...) -> None: ...

class WorkerToApexMessage(_message.Message):
    __slots__ = ("type", "worker_id", "ticket", "manifest", "assignment")
    class MessageType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        PING: _ClassVar[WorkerToApexMessage.MessageType]
        READY: _ClassVar[WorkerToApexMessage.MessageType]
        RETURN_AGENT: _ClassVar[WorkerToApexMessage.MessageType]
    PING: WorkerToApexMessage.MessageType
    READY: WorkerToApexMessage.MessageType
    RETURN_AGENT: WorkerToApexMessage.MessageType
    TYPE_FIELD_NUMBER: _ClassVar[int]
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    TICKET_FIELD_NUMBER: _ClassVar[int]
    MANIFEST_FIELD_NUMBER: _ClassVar[int]
    ASSIGNMENT_FIELD_NUMBER: _ClassVar[int]
    type: WorkerToApexMessage.MessageType
    worker_id: str
    ticket: str
    manifest: WorkerManifest
    assignment: AgentAssignment
    def __init__(self, type: _Optional[_Union[WorkerToApexMessage.MessageType, str]] = ..., worker_id: _Optional[str] = ..., ticket: _Optional[str] = ..., manifest: _Optional[_Union[WorkerManifest, _Mapping]] = ..., assignment: _Optional[_Union[AgentAssignment, _Mapping]] = ...) -> None: ...
