from google.protobuf import empty_pb2 as _empty_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Mapping as _Mapping, Optional as _Optional

DESCRIPTOR: _descriptor.FileDescriptor

class ObjectType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    BLOB: _ClassVar[ObjectType]
    TREE: _ClassVar[ObjectType]
    MESSAGE: _ClassVar[ObjectType]
    MAILBOX: _ClassVar[ObjectType]
    STEP: _ClassVar[ObjectType]
BLOB: ObjectType
TREE: ObjectType
MESSAGE: ObjectType
MAILBOX: ObjectType
STEP: ObjectType

class StoreRequest(_message.Message):
    __slots__ = ("agent_id", "object_id", "data")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    OBJECT_ID_FIELD_NUMBER: _ClassVar[int]
    DATA_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    object_id: bytes
    data: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., object_id: _Optional[bytes] = ..., data: _Optional[bytes] = ...) -> None: ...

class LoadRequest(_message.Message):
    __slots__ = ("agent_id", "object_id")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    OBJECT_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    object_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., object_id: _Optional[bytes] = ...) -> None: ...

class LoadResponse(_message.Message):
    __slots__ = ("agent_id", "object_id", "data")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    OBJECT_ID_FIELD_NUMBER: _ClassVar[int]
    DATA_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    object_id: bytes
    data: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., object_id: _Optional[bytes] = ..., data: _Optional[bytes] = ...) -> None: ...

class SetRefRequest(_message.Message):
    __slots__ = ("agent_id", "ref", "object_id")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    REF_FIELD_NUMBER: _ClassVar[int]
    OBJECT_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    ref: str
    object_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., ref: _Optional[str] = ..., object_id: _Optional[bytes] = ...) -> None: ...

class GetRefRequest(_message.Message):
    __slots__ = ("agent_id", "ref")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    REF_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    ref: str
    def __init__(self, agent_id: _Optional[bytes] = ..., ref: _Optional[str] = ...) -> None: ...

class GetRefResponse(_message.Message):
    __slots__ = ("agent_id", "ref", "object_id")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    REF_FIELD_NUMBER: _ClassVar[int]
    OBJECT_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    ref: str
    object_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., ref: _Optional[str] = ..., object_id: _Optional[bytes] = ...) -> None: ...

class GetRefsRequest(_message.Message):
    __slots__ = ("agent_id", "ref_prefix")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    REF_PREFIX_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    ref_prefix: str
    def __init__(self, agent_id: _Optional[bytes] = ..., ref_prefix: _Optional[str] = ...) -> None: ...

class GetRefsResponse(_message.Message):
    __slots__ = ("agent_id", "refs")
    class RefsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: bytes
        def __init__(self, key: _Optional[str] = ..., value: _Optional[bytes] = ...) -> None: ...
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    REFS_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    refs: _containers.ScalarMap[str, bytes]
    def __init__(self, agent_id: _Optional[bytes] = ..., refs: _Optional[_Mapping[str, bytes]] = ...) -> None: ...
