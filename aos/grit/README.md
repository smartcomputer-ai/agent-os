This contains the Grit object model, the Grit serialization standard, and the standard interfaces to read and write grit objects (grit store and references).

The `stores` submodules are non-server implementations to test Grit and other core functions, but are not used in the runtime. The runtime implements its own Grit storage server (see `runtime/store`).