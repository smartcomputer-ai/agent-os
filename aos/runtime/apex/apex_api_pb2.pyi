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

class InjectMessageRequest(_message.Message):
    __slots__ = ("agent_id", "recipient_id", "message_id", "message_data")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    RECIPIENT_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_DATA_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    recipient_id: bytes
    message_id: bytes
    message_data: InjectMessageData
    def __init__(self, agent_id: _Optional[bytes] = ..., recipient_id: _Optional[bytes] = ..., message_id: _Optional[bytes] = ..., message_data: _Optional[_Union[InjectMessageData, _Mapping]] = ...) -> None: ...

class InjectMessageData(_message.Message):
    __slots__ = ("headers", "is_signal", "content_id", "content_blob")
    class HeadersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    HEADERS_FIELD_NUMBER: _ClassVar[int]
    IS_SIGNAL_FIELD_NUMBER: _ClassVar[int]
    CONTENT_ID_FIELD_NUMBER: _ClassVar[int]
    CONTENT_BLOB_FIELD_NUMBER: _ClassVar[int]
    headers: _containers.ScalarMap[str, str]
    is_signal: bool
    content_id: bytes
    content_blob: bytes
    def __init__(self, headers: _Optional[_Mapping[str, str]] = ..., is_signal: bool = ..., content_id: _Optional[bytes] = ..., content_blob: _Optional[bytes] = ...) -> None: ...

class InjectMessageResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class RunQueryRequest(_message.Message):
    __slots__ = ("agent_id", "actor_id", "query_name", "context")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    QUERY_NAME_FIELD_NUMBER: _ClassVar[int]
    CONTEXT_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    query_name: str
    context: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ..., query_name: _Optional[str] = ..., context: _Optional[bytes] = ...) -> None: ...

class RunQueryResponse(_message.Message):
    __slots__ = ("agent_id", "actor_id", "result_id", "result_blob", "error")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    RESULT_ID_FIELD_NUMBER: _ClassVar[int]
    RESULT_BLOB_FIELD_NUMBER: _ClassVar[int]
    ERROR_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    result_id: bytes
    result_blob: bytes
    error: str
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ..., result_id: _Optional[bytes] = ..., result_blob: _Optional[bytes] = ..., error: _Optional[str] = ...) -> None: ...
