OUT=./lib/protos
mkdir -p $OUT
poetry run python -m grpc_tools.protoc -I./protos --python_out=$OUT --pyi_out=$OUT --grpc_python_out=$OUT ./protos/helloworld.proto