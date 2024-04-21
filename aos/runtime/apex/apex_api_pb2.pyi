from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Iterable as _Iterable, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class StartAgentRequest(_message.Message):
    __slots__ = ("agent_id",)
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ...) -> None: ...

class StartAgentResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class StopAgentRequest(_message.Message):
    __slots__ = ("agent_id",)
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ...) -> None: ...

class StopAgentResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetRunningAgentsRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetRunningAgentsResponse(_message.Message):
    __slots__ = ("agents",)
    AGENTS_FIELD_NUMBER: _ClassVar[int]
    agents: _containers.RepeatedCompositeFieldContainer[AgentInfo]
    def __init__(self, agents: _Optional[_Iterable[_Union[AgentInfo, _Mapping]]] = ...) -> None: ...

class AgentInfo(_message.Message):
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

class GetApexStatusRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetApexStatusResponse(_message.Message):
    __slots__ = ("status", "node_id", "store_address", "workers")
    class ApexStatus(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        UNKNOWN: _ClassVar[GetApexStatusResponse.ApexStatus]
        STARTING: _ClassVar[GetApexStatusResponse.ApexStatus]
        RUNNING: _ClassVar[GetApexStatusResponse.ApexStatus]
        STOPPING: _ClassVar[GetApexStatusResponse.ApexStatus]
        ERROR: _ClassVar[GetApexStatusResponse.ApexStatus]
    UNKNOWN: GetApexStatusResponse.ApexStatus
    STARTING: GetApexStatusResponse.ApexStatus
    RUNNING: GetApexStatusResponse.ApexStatus
    STOPPING: GetApexStatusResponse.ApexStatus
    ERROR: GetApexStatusResponse.ApexStatus
    STATUS_FIELD_NUMBER: _ClassVar[int]
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    STORE_ADDRESS_FIELD_NUMBER: _ClassVar[int]
    WORKERS_FIELD_NUMBER: _ClassVar[int]
    status: GetApexStatusResponse.ApexStatus
    node_id: str
    store_address: str
    workers: _containers.RepeatedCompositeFieldContainer[WorkerInfo]
    def __init__(self, status: _Optional[_Union[GetApexStatusResponse.ApexStatus, str]] = ..., node_id: _Optional[str] = ..., store_address: _Optional[str] = ..., workers: _Optional[_Iterable[_Union[WorkerInfo, _Mapping]]] = ...) -> None: ...

class WorkerInfo(_message.Message):
    __slots__ = ("worker_id", "capabilities", "current_agents")
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
    worker_id: str
    capabilities: _containers.ScalarMap[str, str]
    current_agents: _containers.RepeatedScalarFieldContainer[bytes]
    def __init__(self, worker_id: _Optional[str] = ..., capabilities: _Optional[_Mapping[str, str]] = ..., current_agents: _Optional[_Iterable[bytes]] = ...) -> None: ...
