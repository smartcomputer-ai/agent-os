from google.protobuf import empty_pb2 as _empty_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Mapping as _Mapping, Optional as _Optional

DESCRIPTOR: _descriptor.FileDescriptor

class CreateAgentRequest(_message.Message):
    __slots__ = ("agent_did", "agent_did_private_key")
    AGENT_DID_FIELD_NUMBER: _ClassVar[int]
    AGENT_DID_PRIVATE_KEY_FIELD_NUMBER: _ClassVar[int]
    agent_did: str
    agent_did_private_key: str
    def __init__(self, agent_did: _Optional[str] = ..., agent_did_private_key: _Optional[str] = ...) -> None: ...

class CreateAgentResponse(_message.Message):
    __slots__ = ("agent_id", "agent_did")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    AGENT_DID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    agent_did: str
    def __init__(self, agent_id: _Optional[bytes] = ..., agent_did: _Optional[str] = ...) -> None: ...

class DeleteAgentRequest(_message.Message):
    __slots__ = ("agent_id", "agent_did")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    AGENT_DID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    agent_did: str
    def __init__(self, agent_id: _Optional[bytes] = ..., agent_did: _Optional[str] = ...) -> None: ...

class DeleteAgentResponse(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class GetAgentRequest(_message.Message):
    __slots__ = ("agent_id", "agent_did")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    AGENT_DID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    agent_did: str
    def __init__(self, agent_id: _Optional[bytes] = ..., agent_did: _Optional[str] = ...) -> None: ...

class GetAgentResponse(_message.Message):
    __slots__ = ("agent_id", "agent_did", "exists")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    AGENT_DID_FIELD_NUMBER: _ClassVar[int]
    EXISTS_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    agent_did: str
    exists: bool
    def __init__(self, agent_id: _Optional[bytes] = ..., agent_did: _Optional[str] = ..., exists: bool = ...) -> None: ...

class GetAgentsRequest(_message.Message):
    __slots__ = ("var_filters",)
    class VarFiltersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    VAR_FILTERS_FIELD_NUMBER: _ClassVar[int]
    var_filters: _containers.ScalarMap[str, str]
    def __init__(self, var_filters: _Optional[_Mapping[str, str]] = ...) -> None: ...

class GetAgentsResponse(_message.Message):
    __slots__ = ("agents",)
    class AgentsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: bytes
        def __init__(self, key: _Optional[str] = ..., value: _Optional[bytes] = ...) -> None: ...
    AGENTS_FIELD_NUMBER: _ClassVar[int]
    agents: _containers.ScalarMap[str, bytes]
    def __init__(self, agents: _Optional[_Mapping[str, bytes]] = ...) -> None: ...

class SetVarRequest(_message.Message):
    __slots__ = ("agent_id", "key", "value")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key: str
    value: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...

class GetVarRequest(_message.Message):
    __slots__ = ("agent_id", "key")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key: _Optional[str] = ...) -> None: ...

class GetVarResponse(_message.Message):
    __slots__ = ("agent_id", "key", "value")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key: str
    value: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...

class GetVarsRequest(_message.Message):
    __slots__ = ("agent_id", "key_prefix")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_PREFIX_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key_prefix: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key_prefix: _Optional[str] = ...) -> None: ...

class GetVarsResponse(_message.Message):
    __slots__ = ("agent_id", "vars")
    class VarsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: bytes
        def __init__(self, key: _Optional[str] = ..., value: _Optional[bytes] = ...) -> None: ...
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    VARS_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    vars: _containers.ScalarMap[str, bytes]
    def __init__(self, agent_id: _Optional[bytes] = ..., vars: _Optional[_Mapping[str, bytes]] = ...) -> None: ...

class DeleteVarRequest(_message.Message):
    __slots__ = ("agent_id", "key")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key: _Optional[str] = ...) -> None: ...
