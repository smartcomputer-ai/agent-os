# Fabric Requirements

> Maintained by the user

### Now
- get access to a unix machine to run commands
- run/exec commands as rpc or jobs (jobs later)
- rpc: immediate feedback, stream back outputs (important if longer running), multiple calls at the same time allowed
- sessions run from a few seconds/minutes to days/weeks
- quiesce sessions allowed
- currently not meant to deploy software permanently, more as an agent dev scratchpad
- check out repositories
- work with rust, python, node, go, etc
- install new/missing tools inside a session (e.g. install via curl or apt-get)
- semi-permanent volumes: not the end of the world if lost. do not have to survive sessions
- sessions are standalone (no cross-session communication needed)
- no docker inside sessions itself
- local sessions (access local machine), but not always enabled

### Later:
- SSH connections/sessions
- deploy software permanently (pods, services, etc)
- volumes survive sessions, can be re-used across sessions
- volumes can be shared between sessions
- desktop sessions
- browser sessions
- sessions can communicate (e..g one session runs a db, the other backen, the other the UI)
- docker inside sessions (e.g. run docker compose to test stuff)
- jobs: one job runs against any given session at a time, can be long running, can take a while


## Fabric Runtime Options
- Containers
- Firecracker VMs
- Normal VMs

Hundreds to thousands of sessions will exist at any time. But since they are mainly used for dev. Many sessions do not always need to be live.

Fabric should coordinate the sessions and underlying compute resources. Sessions should probably _not_ be k8s pods, but rather be managed by the fabric coordinator. A host VM that hosts sessions, _could_ be a k8s resource in the hosted system.

Right now, I'm thinking we should start with container based sessions and then later expand to firecracker. Also, I'm not sure how hard it would be to support SSH sessions in order to connect to any unix server?
