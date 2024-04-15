from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Optional as _Optional

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

class StoreResponse(_message.Message):
    __slots__ = ("agent_id", "object_id")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    OBJECT_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    object_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ..., object_id: _Optional[bytes] = ...) -> None: ...

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
