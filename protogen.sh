OUT=./
mkdir -p $OUT
poetry run python -m grpc_tools.protoc -I./protos --python_out=$OUT --pyi_out=$OUT --grpc_python_out=$OUT ./protos/aos/runtime/store/grit_store.proto
poetry run python -m grpc_tools.protoc -I./protos --python_out=$OUT --pyi_out=$OUT --grpc_python_out=$OUT ./protos/aos/runtime/apex/apex.proto