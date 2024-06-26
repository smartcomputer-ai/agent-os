# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# source: aos/runtime/worker/worker_api.proto
# Protobuf Python Version: 4.25.1
"""Generated protocol buffer code."""
from google.protobuf import descriptor as _descriptor
from google.protobuf import descriptor_pool as _descriptor_pool
from google.protobuf import symbol_database as _symbol_database
from google.protobuf.internal import builder as _builder
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()




DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n#aos/runtime/worker/worker_api.proto\"\x85\x01\n\x14InjectMessageRequest\x12\x10\n\x08\x61gent_id\x18\x01 \x01(\x0c\x12\x14\n\x0crecipient_id\x18\x02 \x01(\x0c\x12\x14\n\nmessage_id\x18\x05 \x01(\x0cH\x00\x12$\n\x0cmessage_data\x18\x06 \x01(\x0b\x32\x0c.MessageDataH\x00\x42\t\n\x07message\"\xca\x01\n\x0bMessageData\x12*\n\x07headers\x18\x03 \x03(\x0b\x32\x19.MessageData.HeadersEntry\x12\x11\n\tis_signal\x18\x04 \x01(\x08\x12\x13\n\x0bprevious_id\x18\x05 \x01(\x0c\x12\x14\n\ncontent_id\x18\n \x01(\x0cH\x00\x12\x16\n\x0c\x63ontent_blob\x18\x0b \x01(\x0cH\x00\x1a.\n\x0cHeadersEntry\x12\x0b\n\x03key\x18\x01 \x01(\t\x12\r\n\x05value\x18\x02 \x01(\t:\x02\x38\x01\x42\t\n\x07\x63ontent\"=\n\x15InjectMessageResponse\x12\x10\n\x08\x61gent_id\x18\x01 \x01(\x0c\x12\x12\n\nmessage_id\x18\x02 \x01(\x0c\"k\n\x0fRunQueryRequest\x12\x10\n\x08\x61gent_id\x18\x01 \x01(\x0c\x12\x10\n\x08\x61\x63tor_id\x18\x02 \x01(\x0c\x12\x12\n\nquery_name\x18\x04 \x01(\t\x12\x14\n\x07\x63ontext\x18\x05 \x01(\x0cH\x00\x88\x01\x01\x42\n\n\x08_context\"V\n\x10RunQueryResponse\x12\x10\n\x08\x61gent_id\x18\x01 \x01(\x0c\x12\x10\n\x08\x61\x63tor_id\x18\x02 \x01(\x0c\x12\x13\n\x06result\x18\n \x01(\x0cH\x00\x88\x01\x01\x42\t\n\x07_result\"\'\n\x13SubscriptionRequest\x12\x10\n\x08\x61gent_id\x18\x01 \x01(\x0c\"r\n\x13SubscriptionMessage\x12\x10\n\x08\x61gent_id\x18\x01 \x01(\x0c\x12\x11\n\tsender_id\x18\x02 \x01(\x0c\x12\x12\n\nmessage_id\x18\x03 \x01(\x0c\x12\"\n\x0cmessage_data\x18\x04 \x01(\x0b\x32\x0c.MessageData2\xc4\x01\n\tWorkerApi\x12@\n\rInjectMessage\x12\x15.InjectMessageRequest\x1a\x16.InjectMessageResponse\"\x00\x12\x31\n\x08RunQuery\x12\x10.RunQueryRequest\x1a\x11.RunQueryResponse\"\x00\x12\x42\n\x10SubscribeToAgent\x12\x14.SubscriptionRequest\x1a\x14.SubscriptionMessage\"\x00\x30\x01\x62\x06proto3')

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'aos.runtime.worker.worker_api_pb2', _globals)
if _descriptor._USE_C_DESCRIPTORS == False:
  DESCRIPTOR._options = None
  _globals['_MESSAGEDATA_HEADERSENTRY']._options = None
  _globals['_MESSAGEDATA_HEADERSENTRY']._serialized_options = b'8\001'
  _globals['_INJECTMESSAGEREQUEST']._serialized_start=40
  _globals['_INJECTMESSAGEREQUEST']._serialized_end=173
  _globals['_MESSAGEDATA']._serialized_start=176
  _globals['_MESSAGEDATA']._serialized_end=378
  _globals['_MESSAGEDATA_HEADERSENTRY']._serialized_start=321
  _globals['_MESSAGEDATA_HEADERSENTRY']._serialized_end=367
  _globals['_INJECTMESSAGERESPONSE']._serialized_start=380
  _globals['_INJECTMESSAGERESPONSE']._serialized_end=441
  _globals['_RUNQUERYREQUEST']._serialized_start=443
  _globals['_RUNQUERYREQUEST']._serialized_end=550
  _globals['_RUNQUERYRESPONSE']._serialized_start=552
  _globals['_RUNQUERYRESPONSE']._serialized_end=638
  _globals['_SUBSCRIPTIONREQUEST']._serialized_start=640
  _globals['_SUBSCRIPTIONREQUEST']._serialized_end=679
  _globals['_SUBSCRIPTIONMESSAGE']._serialized_start=681
  _globals['_SUBSCRIPTIONMESSAGE']._serialized_end=795
  _globals['_WORKERAPI']._serialized_start=798
  _globals['_WORKERAPI']._serialized_end=994
# @@protoc_insertion_point(module_scope)
