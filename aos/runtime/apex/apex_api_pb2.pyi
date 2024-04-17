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
    __slots__ = ("agent_id", "agent_did")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    AGENT_DID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    agent_did: str
    def __init__(self, agent_id: _Optional[bytes] = ..., agent_did: _Optional[str] = ...) -> None: ...

class GetApexStatusRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetApexStatusResponse(_message.Message):
    __slots__ = ("grit_address", "workers")
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
    GRIT_ADDRESS_FIELD_NUMBER: _ClassVar[int]
    WORKERS_FIELD_NUMBER: _ClassVar[int]
    grit_address: str
    workers: _containers.RepeatedCompositeFieldContainer[WorkerInfo]
    def __init__(self, grit_address: _Optional[str] = ..., workers: _Optional[_Iterable[_Union[WorkerInfo, _Mapping]]] = ...) -> None: ...

class WorkerInfo(_message.Message):
    __slots__ = ("worker_id", "capabilities")
    class CapabilitiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    CAPABILITIES_FIELD_NUMBER: _ClassVar[int]
    worker_id: str
    capabilities: _containers.ScalarMap[str, str]
    def __init__(self, worker_id: _Optional[str] = ..., capabilities: _Optional[_Mapping[str, str]] = ...) -> None: ...

class InjectMessageRequest(_message.Message):
    __slots__ = ("agent_id", "recipient_id", "headers", "is_signal", "content_id", "content_blob")
    class HeadersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    RECIPIENT_ID_FIELD_NUMBER: _ClassVar[int]
    HEADERS_FIELD_NUMBER: _ClassVar[int]
    IS_SIGNAL_FIELD_NUMBER: _ClassVar[int]
    CONTENT_ID_FIELD_NUMBER: _ClassVar[int]
    CONTENT_BLOB_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    recipient_id: bytes
    headers: _containers.ScalarMap[str, str]
    is_signal: bool
    content_id: bytes
    content_blob: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., recipient_id: _Optional[bytes] = ..., headers: _Optional[_Mapping[str, str]] = ..., is_signal: bool = ..., content_id: _Optional[bytes] = ..., content_blob: _Optional[bytes] = ...) -> None: ...

class InjectMessageResponse(_message.Message):
    __slots__ = ("agent_id", "message_id")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    message_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., message_id: _Optional[bytes] = ...) -> None: ...

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
    __slots__ = ("agent_id", "actor_id", "tree_id", "blob")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    TREE_ID_FIELD_NUMBER: _ClassVar[int]
    BLOB_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    tree_id: bytes
    blob: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ..., tree_id: _Optional[bytes] = ..., blob: _Optional[bytes] = ...) -> None: ...
