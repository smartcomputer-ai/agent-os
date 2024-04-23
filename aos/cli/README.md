# Agent OS CLI

## How to Install
In `/usr/local/bin` create a script called `aos` with the following content:
```bash
poetry -C "/home/pathtorepo/dev/agent-os/" run aos "$@"
```