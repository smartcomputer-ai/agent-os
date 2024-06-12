
import os

from aos.grit.object_model import *
from aos.grit.object_serialization import *


# random test data
def get_random_object_id() -> ObjectId:
    return get_object_id(os.urandom(32))

def get_random_blob(headers:dict[str,str]=None) -> Blob:
    return Blob(headers, os.urandom(1024))

def get_random_tree() -> Tree:
    return {str(i): get_random_object_id() for i in range(100)}

def get_random_message(headers:dict=None) -> Message:
    return Message(
        get_random_object_id(),
        headers,
        get_random_object_id())

def get_random_mailbox() -> Mailbox:
    return {get_random_object_id(): get_random_object_id() for i in range(100)}

def get_random_step() -> Step:
    return Step(
        get_random_object_id(),
        get_random_object_id(),
        get_random_object_id(),
        get_random_object_id(),
        get_random_object_id())

# tests
def test_blob():
    test_data = get_random_blob()
    b = blob_to_bytes(test_data)
    test_data_2 = bytes_to_blob(b)
    print(test_data)
    print(test_data_2)
    assert test_data == test_data_2

def test_blob_with_headers():
    test_data = get_random_blob({'hi': 'there', 'foo': 'bar'})
    b = blob_to_bytes(test_data)
    test_data_2 = bytes_to_blob(b)
    print(test_data)
    print(test_data_2)
    assert test_data == test_data_2

def test_tree():
    test_data = get_random_tree()
    b = tree_to_bytes(test_data)
    test_data_2 = bytes_to_tree(b)
    assert test_data == test_data_2

def test_message():
    test_data = get_random_message()
    b = message_to_bytes(test_data)
    test_data_2 = bytes_to_message(b)
    assert test_data == test_data_2

def test_message_with_no_previous():
    test_data = Message(
        None,
        None,
        get_random_object_id())
    b = message_to_bytes(test_data)
    test_data_2 = bytes_to_message(b)
    assert test_data == test_data_2

def test_message_with_headers():
    test_data = get_random_message({'hi': 'there', 'foo': 'bar'})
    b = message_to_bytes(test_data)
    test_data_2 = bytes_to_message(b)
    assert test_data == test_data_2

def test_mailbox():
    test_data = get_random_mailbox()
    b = mailbox_to_bytes(test_data)
    test_data_2 = bytes_to_mailbox(b)
    assert test_data == test_data_2

def test_step():
    test_data = get_random_step()
    b = step_to_bytes(test_data)
    test_data_2 = bytes_to_step(b)
    assert test_data == test_data_2

def test_step_with_no_previous():
    test_data = Step(
        None,
        get_random_object_id(),
        get_random_object_id(),
        get_random_object_id(),
        get_random_object_id())
    b = step_to_bytes(test_data)
    test_data_2 = bytes_to_step(b)
    assert test_data == test_data_2

def test_deserialize_type_mismatch():
    test_data = get_random_tree()
    b = tree_to_bytes(test_data)
    try:
        bytes_to_message(b)
        assert False
    except TypeError:
        assert True

def test_object_to_bytes_and_back():
    test_data = get_random_step()
    b = object_to_bytes(test_data)
    test_data_2 = bytes_to_object(b)
    assert test_data == test_data_2

    test_data = get_random_tree()
    b = object_to_bytes(test_data)
    test_data_2 = bytes_to_object(b)
    assert test_data == test_data_2

    test_data = get_random_mailbox()
    b = object_to_bytes(test_data)
    test_data_2 = bytes_to_object(b)
    assert test_data == test_data_2

    test_data = get_random_blob()
    b = object_to_bytes(test_data)
    test_data_2 = bytes_to_object(b)
    assert test_data == test_data_2

    test_data = get_random_blob({'hi': 'there', 'foo': 'bar'})
    b = object_to_bytes(test_data)
    test_data_2 = bytes_to_object(b)
    assert test_data == test_data_2

