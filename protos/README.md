# Protocol Buffers Definitions

For IPC between grit, apex, workers, and the webserver (and CLI).

The folder structure of the proto definitions MUST match the desired output location and module name. Because protoc generates only absolute imports, which do not work unles they match the aos module structure.

See: https://github.com/protocolbuffers/protobuf/issues/1491

## Reference

- https://github.com/codethecoffee/proto-cheatsheet
- https://protobuf.dev/programming-guides/proto3/