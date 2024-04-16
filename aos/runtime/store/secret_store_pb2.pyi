from google.protobuf import empty_pb2 as _empty_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Mapping as _Mapping, Optional as _Optional

DESCRIPTOR: _descriptor.FileDescriptor

class SetSecretRequest(_message.Message):
    __slots__ = ("agent_id", "key", "value")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key: str
    value: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...

class GetSecretRequest(_message.Message):
    __slots__ = ("agent_id", "key")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key: _Optional[str] = ...) -> None: ...

class GetSecretResponse(_message.Message):
    __slots__ = ("agent_id", "key", "value")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key: str
    value: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...

class GetSecretsRequest(_message.Message):
    __slots__ = ("agent_id", "key_prefix")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_PREFIX_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key_prefix: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key_prefix: _Optional[str] = ...) -> None: ...

class GetSecretsResponse(_message.Message):
    __slots__ = ("agent_id", "values")
    class ValuesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: bytes
        def __init__(self, key: _Optional[str] = ..., value: _Optional[bytes] = ...) -> None: ...
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    VALUES_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    values: _containers.ScalarMap[str, bytes]
    def __init__(self, agent_id: _Optional[bytes] = ..., values: _Optional[_Mapping[str, bytes]] = ...) -> None: ...

class DeleteSecretRequest(_message.Message):
    __slots__ = ("agent_id", "key")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    KEY_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    key: str
    def __init__(self, agent_id: _Optional[bytes] = ..., key: _Optional[str] = ...) -> None: ...
