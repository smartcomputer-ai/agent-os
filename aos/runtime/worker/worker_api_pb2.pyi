from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class InjectMessageRequest(_message.Message):
    __slots__ = ("agent_id", "recipient_id", "message_id", "message_data")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    RECIPIENT_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_DATA_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    recipient_id: bytes
    message_id: bytes
    message_data: MessageData
    def __init__(self, agent_id: _Optional[bytes] = ..., recipient_id: _Optional[bytes] = ..., message_id: _Optional[bytes] = ..., message_data: _Optional[_Union[MessageData, _Mapping]] = ...) -> None: ...

class MessageData(_message.Message):
    __slots__ = ("headers", "is_signal", "previous_id", "content_id", "content_blob")
    class HeadersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    HEADERS_FIELD_NUMBER: _ClassVar[int]
    IS_SIGNAL_FIELD_NUMBER: _ClassVar[int]
    PREVIOUS_ID_FIELD_NUMBER: _ClassVar[int]
    CONTENT_ID_FIELD_NUMBER: _ClassVar[int]
    CONTENT_BLOB_FIELD_NUMBER: _ClassVar[int]
    headers: _containers.ScalarMap[str, str]
    is_signal: bool
    previous_id: bytes
    content_id: bytes
    content_blob: bytes
    def __init__(self, headers: _Optional[_Mapping[str, str]] = ..., is_signal: bool = ..., previous_id: _Optional[bytes] = ..., content_id: _Optional[bytes] = ..., content_blob: _Optional[bytes] = ...) -> None: ...

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
    __slots__ = ("agent_id", "actor_id", "object_id", "object_blob", "error")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    ACTOR_ID_FIELD_NUMBER: _ClassVar[int]
    OBJECT_ID_FIELD_NUMBER: _ClassVar[int]
    OBJECT_BLOB_FIELD_NUMBER: _ClassVar[int]
    ERROR_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    actor_id: bytes
    object_id: bytes
    object_blob: bytes
    error: str
    def __init__(self, agent_id: _Optional[bytes] = ..., actor_id: _Optional[bytes] = ..., object_id: _Optional[bytes] = ..., object_blob: _Optional[bytes] = ..., error: _Optional[str] = ...) -> None: ...

class SubscriptionRequest(_message.Message):
    __slots__ = ("agent_id",)
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    def __init__(self, agent_id: _Optional[bytes] = ...) -> None: ...

class SubscriptionMessage(_message.Message):
    __slots__ = ("agent_id", "sender_id", "message_id", "message_data")
    AGENT_ID_FIELD_NUMBER: _ClassVar[int]
    SENDER_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_DATA_FIELD_NUMBER: _ClassVar[int]
    agent_id: bytes
    sender_id: bytes
    message_id: bytes
    message_data: MessageData
    def __init__(self, agent_id: _Optional[bytes] = ..., sender_id: _Optional[bytes] = ..., message_id: _Optional[bytes] = ..., message_data: _Optional[_Union[MessageData, _Mapping]] = ...) -> None: ...
