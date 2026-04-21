# smolvm on macOS

This is the known-good setup for running Fabric's smolvm runtime on an Apple
Silicon MacBook.

## Host Requirements

Install the host tools:

```sh
brew install e2fsprogs git-lfs
```

`e2fsprogs` provides `mkfs.ext4`, which smolvm uses to format the storage and
overlay disks. Homebrew keeps it keg-only, but smolvm already searches the
Homebrew keg paths directly.

`git-lfs` is useful for the smolvm submodule, but Fabric does not rely on LFS
for local libkrun artifacts. The upstream smolvm LFS budget may be exhausted,
so use the release bundle flow below.

## Stage the smolvm Release Bundle

The source dependency is `third_party/smolvm`, but local runtime artifacts come
from the matching smolvm release archive:

```sh
bash dev/fabric/bootstrap-smolvm-release.sh
```

This creates the ignored directory:

```text
third_party/smolvm-release/
  agent-rootfs
  lib/libkrun.dylib
  lib/libkrunfw.5.dylib
```

When running the smolvm-backed host directly, point the process at this bundle:

```sh
export LIBKRUN_BUNDLE="$PWD/third_party/smolvm-release/lib"
export DYLD_LIBRARY_PATH="$PWD/third_party/smolvm-release/lib"
export SMOLVM_AGENT_ROOTFS="$PWD/third_party/smolvm-release/agent-rootfs"
```

Normal AOS builds do not enable the smolvm runtime by default, so plain
`cargo build` and `cargo build -p fabric-host` do not require the runtime
bundle. To compile the real smolvm-backed host directly, use:

```sh
cargo build -p fabric-host --features smolvm-runtime
```

On macOS, that feature links `libkrun.dylib`. Cargo automatically checks the
staged `third_party/smolvm-release/lib` bundle, or you can override it with
`LIBKRUN_BUNDLE`.

## Build and Sign `fabric-host`

On macOS, the binary that enters libkrun needs the same entitlements smolvm
uses:

```sh
dev/fabric/build-fabric-host.sh
```

Use this after each rebuild of the smolvm-backed `fabric-host`. The script runs
`cargo build -p fabric-host --features smolvm-runtime` and signs the resulting
binary with `third_party/smolvm/smolvm.entitlements`.
`cargo run` is not ideal for VM smoke tests because Cargo may rebuild the binary
and launch an unsigned file. Prefer this script, then run `target/debug/fabric-host`
directly.

## Run the Host Daemon

```sh
RUST_LOG=info \
LIBKRUN_BUNDLE="$PWD/third_party/smolvm-release/lib" \
DYLD_LIBRARY_PATH="$PWD/third_party/smolvm-release/lib" \
SMOLVM_AGENT_ROOTFS="$PWD/third_party/smolvm-release/agent-rootfs" \
target/debug/fabric-host \
  --bind 127.0.0.1:8791 \
  --state-root .fabric-host \
  --host-id local-dev
```

Health check:

```sh
curl -fsS http://127.0.0.1:8791/healthz
```

## Smoke Test a Fabric VM

Open a session with egress enabled so the agent can pull the OCI image:

```sh
curl -fsS -X POST http://127.0.0.1:8791/v1/sessions \
  -H 'content-type: application/json' \
  -d '{
    "session_id": "sess-smoke",
    "image": "docker.io/library/alpine:latest",
    "runtime_class": "smolvm",
    "network_mode": "egress",
    "resources": {
      "cpu_limit_millis": 1000,
      "memory_limit_bytes": 536870912
    }
  }'
```

Expected response:

```json
{"session_id":"sess-smoke","status":"ready","workdir":"/workspace","host_id":"local-dev"}
```

Then check status:

```sh
curl -fsS http://127.0.0.1:8791/v1/sessions/sess-smoke
```

Expected response:

```json
{"session_id":"sess-smoke","status":"ready"}
```

At this point Fabric can boot and keep a smolvm VM running.

Run a non-interactive command:

```sh
target/debug/fabric --endpoint http://127.0.0.1:8791 \
  exec sess-smoke -- uname -a
```

Stop and resume the session:

```sh
target/debug/fabric --endpoint http://127.0.0.1:8791 signal sess-smoke quiesce
target/debug/fabric --endpoint http://127.0.0.1:8791 signal sess-smoke resume
```

Close the session:

```sh
target/debug/fabric --endpoint http://127.0.0.1:8791 signal sess-smoke close
```

## Run the E2E Test

The smolvm integration test is opt-in because it boots a real VM:

```sh
dev/fabric/test-smolvm-e2e.sh
```

The script builds and signs `target/debug/fabric-host`, sets
`FABRIC_HOST_BIN`, enables `FABRIC_SMOLVM_E2E=1`, and runs:

```sh
cargo test -p fabric-host --test smolvm_e2e -- --nocapture
```

Override the image if needed:

```sh
FABRIC_SMOLVM_TEST_IMAGE=docker.io/library/alpine:latest dev/fabric/test-smolvm-e2e.sh
```

## Known Failure Modes

`mkfs.ext4 not found`

Install `e2fsprogs`:

```sh
brew install e2fsprogs
```

`agent rootfs not found: ~/Library/Application Support/smolvm/agent-rootfs`

The daemon was run without `SMOLVM_AGENT_ROOTFS`. Run it with the environment
block shown above.

`Library not loaded: @rpath/libkrun.dylib`

The daemon binary was linked without the local libkrun rpath, or it is being run
outside the expected build environment. Re-run:

```sh
bash dev/fabric/bootstrap-smolvm-release.sh
dev/fabric/build-fabric-host.sh
```

`agent process exited during startup` with a macOS crash report pointing at
`close_inherited_fds`

This means the hidden `_boot-vm` subprocess started with runtime worker threads.
Fabric's `fabric-host` main must dispatch `_boot-vm` before creating any Tokio
runtime.

Session opens as `ready`, but immediate status is `quiesced`

The smolvm `AgentManager` was dropped without `detach()`, so it stopped the VM
after startup. Fabric sessions must call `manager.detach()` after the VM is
ready and the requested image has been pulled.

## Cleanup During Development

If a dev run is interrupted before Fabric can close a session, first find the
smolvm machine name:

```sh
find ~/Library/Caches/smolvm/vms -maxdepth 2 -name name -print -exec sed -n '1p' {} \;
```

Then stop it with the smolvm release binary:

```sh
SMOLVM_AGENT_ROOTFS="$PWD/third_party/smolvm-release/agent-rootfs" \
DYLD_LIBRARY_PATH="$PWD/third_party/smolvm-release/lib" \
third_party/smolvm-release/smolvm machine stop --name <machine-name>
```

Local Fabric daemon state lives under `.fabric-host`. smolvm VM disks live under
`~/Library/Caches/smolvm/vms`.
