import hashlib
import string
from grit.object_model import *

_STR_ENCODING = 'ascii'
_ID_LEN = 32
_ID_STR_LEN = 64

def get_object_id(bytes:bytes | bytearray) -> ObjectId:
    return hashlib.sha256(bytes).digest()

def is_object_id_str(object_id_str:str) -> bool:
    return isinstance(object_id_str, str) and len(object_id_str) == _ID_STR_LEN and all(c in string.hexdigits for c in object_id_str)

def is_object_id(object_id:ObjectId) -> bool:
    return (isinstance(object_id, bytes) or isinstance(object_id, bytearray)) and len(object_id) == _ID_LEN

def to_object_id_str(object_id:ObjectId) -> str:
    return object_id.hex()

def to_object_id(object_id_str:str) -> ObjectId:
    return bytes.fromhex(object_id_str)

def is_blob(object:Object) -> bool:
    return isinstance(object, Blob) or type(object).__name__ == 'Blob'

def is_message(object:Object) -> bool:
    return isinstance(object, Message) or type(object).__name__ == 'Message'

def is_step(object:Object) -> bool:
    return isinstance(object, Step) or type(object).__name__ == 'Step'

def is_tree(object:Object) -> bool:
    return isinstance(object, dict) and len(object) > 0 and isinstance(next(iter(object)), str) and isinstance(object[next(iter(object))], ObjectId)

def is_mailbox(object:Object) -> bool:
    return (isinstance(object, dict) and len(object) > 0 and (isinstance(next(iter(object)), bytes) or isinstance(next(iter(object)), bytearray)) 
        and (isinstance(object[next(iter(object))], bytes) or isinstance(object[next(iter(object))], bytearray)))

def object_to_bytes(object:Object) -> bytes:
    if is_blob(object):
        return blob_to_bytes(object)
    elif is_message(object):
        return message_to_bytes(object)
    elif is_step(object):
        return step_to_bytes(object)
    elif is_tree(object):
        return tree_to_bytes(object)
    elif is_mailbox(object):
        return mailbox_to_bytes(object)
    else:
        raise TypeError("Unknown object type")

def bytes_to_object(bytes) -> Object:
    object_type, _ = _peek_object_header(bytes)
    if object_type == 'blob':
        return bytes_to_blob(bytes)
    elif object_type == 'tree':
        return bytes_to_tree(bytes)
    elif object_type == 'message':
        return bytes_to_message(bytes)
    elif object_type == 'mailbox':
        return bytes_to_mailbox(bytes)
    elif object_type == 'step':
        return bytes_to_step(bytes)
    else:
        raise TypeError("Unknown object type")

def _object_header_to_bytes(object_type:str, length:int) -> bytearray:
    return bytearray(f"{object_type} {length}\x00".encode(_STR_ENCODING))

def _peek_object_header(bytes:bytes) -> tuple[str, int]:
    header = bytes[:bytes.find(b'\x00')]
    header_str = header.decode(_STR_ENCODING)
    object_type, length_str = header_str.split(' ')
    return object_type, int(length_str)

def _enforce_and_skip_object_header(bytes:bytes, expected_object_type:str) -> bytes:
    header, body = bytes.split(b'\x00', 1)
    header_str = header.decode(_STR_ENCODING)
    object_type, length_str = header_str.split(' ')
    if object_type != expected_object_type:
        raise TypeError(f"Expected {expected_object_type} but got {object_type}")
    length = int(length_str)
    if len(body) != length:
        raise Exception(f"Expected object body of {length} bytes but got {len(body)}")
    return body

def _enforce_object_id(object_id:ObjectId) -> ObjectId:
    if not isinstance(object_id, bytes):
        raise TypeError(f"Expected object id of type bytes but got {type(object_id)}")
    if len(object_id) != _ID_LEN:
        raise ValueError(f"Expected object id of {_ID_LEN} bytes but got {len(object_id)}")
    #check that they the array is not all \x00
    if all(byte == 0 for byte in object_id):
        raise ValueError("Expected object id to not be all \x00")
    return object_id

def _split_bytes(bytes:bytes, n:int) -> tuple[bytes, bytes]:
    return bytes[:n], bytes[n:]

def _write_internal_headers(headers:dict[str, str], result:bytearray) -> bytearray:
    if(headers is not None):
        for key, value in headers.items():
            if(value is None):
                continue
            result += key.encode(_STR_ENCODING)
            result += b'\x00'
            result += value.encode(_STR_ENCODING)
            result += b'\x00'
    result += b'\x00'
    return result

def _read_internal_headers(bytes:bytes) -> tuple[dict[str, str], bytes]:
    headers = {}
    while len(bytes) > 0:
        header_name, bytes = bytes.split(b'\x00', 1)
        if(len(header_name) == 0):
            break
        heaver_value, bytes = bytes.split(b'\x00', 1)
        headers[header_name.decode(_STR_ENCODING)] = heaver_value.decode(_STR_ENCODING)
    if(headers == {}):
        headers = None
    return headers, bytes

def blob_to_bytes(object:Blob) -> bytes:
    result = bytearray()
    result = _write_internal_headers(object.headers, result)
    result.extend(object.data)
    objec_header = _object_header_to_bytes('blob', len(result))
    return bytes(objec_header + result)

def bytes_to_blob(bytes) -> Blob:
    bytes = _enforce_and_skip_object_header(bytes, 'blob')
    headers, bytes = _read_internal_headers(bytes)
    data = bytes #the remaining bytes are the data
    return Blob(headers, data)

def tree_to_bytes(object:Tree) -> bytes:
    result = bytearray()
    for key, value in object.items():
        result += key.encode(_STR_ENCODING)
        result += b'\x00'
        result += _enforce_object_id(value)
    objec_header = _object_header_to_bytes('tree', len(result))
    return bytes(objec_header + result)

def bytes_to_tree(bytes) -> Tree:
    bytes = _enforce_and_skip_object_header(bytes, 'tree')
    result = {}
    while len(bytes) > 0:
        name, bytes = bytes.split(b'\x00', 1)
        object_id, bytes = _split_bytes(bytes, _ID_LEN)
        result[name.decode(_STR_ENCODING)] = _enforce_object_id(object_id)
    return result

def message_to_bytes(object:Message) -> bytes:
    result = bytearray()
    if object.previous is None:
        result += bytes(_ID_LEN) # all \x00
    else:
        result += _enforce_object_id(object.previous)
    result = _write_internal_headers(object.headers, result)
    result += _enforce_object_id(object.content)
    objec_header = _object_header_to_bytes('message', len(result))
    return bytes(objec_header + result)

def bytes_to_message(bytes) -> Message:
    bytes = _enforce_and_skip_object_header(bytes, 'message')
    previous, bytes = _split_bytes(bytes, _ID_LEN)
    headers, bytes = _read_internal_headers(bytes)
    content, bytes = _split_bytes(bytes, _ID_LEN)
    return Message(
        None if all(byte == 0 for byte in previous) else _enforce_object_id(previous),
        headers,
        _enforce_object_id(content))

def mailbox_to_bytes(object:Mailbox) -> bytes:
    result = bytearray()
    for key, value in object.items():
        result += _enforce_object_id(key)
        result += _enforce_object_id(value)
    objec_header = _object_header_to_bytes('mailbox', len(result))
    return bytes(objec_header + result)

def bytes_to_mailbox(bytes) -> Mailbox:
    bytes = _enforce_and_skip_object_header(bytes, 'mailbox')
    result = {}
    while len(bytes) > 0:
        key, bytes = _split_bytes(bytes, _ID_LEN)
        value, bytes = _split_bytes(bytes, _ID_LEN)
        result[_enforce_object_id(key)] = _enforce_object_id(value)
    return result

def step_to_bytes(object:Step) -> bytes:
    result = bytearray()
    if object.previous is None:
        result += bytes(_ID_LEN) # all \x00
    else:
        result += _enforce_object_id(object.previous)
    result += _enforce_object_id(object.actor)
    if object.inbox is None:
        result += bytes(_ID_LEN) # all \x00
    else:
        result += _enforce_object_id(object.inbox)
    if object.outbox is None:
        result += bytes(_ID_LEN) # all \x00
    else:
        result += _enforce_object_id(object.outbox)
    result += _enforce_object_id(object.core)
    objec_header = _object_header_to_bytes('step', len(result))
    return bytes(objec_header + result)

def bytes_to_step(bytes) -> Step:
    bytes = _enforce_and_skip_object_header(bytes, 'step')
    previous, bytes = _split_bytes(bytes, _ID_LEN)
    actor, bytes = _split_bytes(bytes, _ID_LEN)
    inbox, bytes = _split_bytes(bytes, _ID_LEN)
    outbox, bytes = _split_bytes(bytes, _ID_LEN)
    core, bytes = _split_bytes(bytes, _ID_LEN)
    return Step(
        None if all(byte == 0 for byte in previous) else _enforce_object_id(previous),
        _enforce_object_id(actor),
        None if all(byte == 0 for byte in inbox) else _enforce_object_id(inbox),
        None if all(byte == 0 for byte in outbox) else _enforce_object_id(outbox),
        _enforce_object_id(core))