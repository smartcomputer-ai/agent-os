
## Grit

 - send objects
 - retrieve object
 - set refs (should that be here?, maybe just for runtime use)
 - multi-tenant support
 - stream larger objects


## Worker
 
 - figure out what manifests are supported on this worker
 - request work from orch. -> which actors to run
 - run (some) OS level actors (? or should they only exist once?, but that would mean they dont have an actor id... that would be ok)
 - timers (should the worker do that?)
 - send messages to orch. ... ideally send messages straight to other worker where actor is p2p. or route to local ones (could be future work)
 - route messages internally
 - backoff of delivering messages if actor does not accept (stop trying at some point? but then how to persist that mismatch)
 - listen to messages for its own actors
 - set step head for actors (when messages are sent off? or before?)
 - keep track of performance of actors and general resource usage, to message orch when re-balancing is needed
 - keep a local grit cache
 - run queries
 - ask the orch. for which Grit to use (if Grit is sharded)


## Orchestrator

 - aware of all actors
 - aware of manifests of all actors
 - decide which actor to run on which worker
 - route messages between workers, as long as there is no p2p solution yet
 - re-route if worker goes down/offline
 - warn if no worker exists for manifest
 - restart actor messaging after complete shutdown (tradeoff, to allow worker to either set the step head before sending all mesages, or having to wait. with the former, we can pesist undelivered messages in the orch. and then start from there without re-analyzing the entire message state.)
 - initiate pruning
 - snapshot all heads for an agent (before updates), refert to certain snapshots
 - initiate updates
 - host web server (could be different service) and route queries and messages to workers


## Structure

We'll implement the first version as a monolith that can be started with different settings.

All of it will be in python.

- protos
- src
  - shared
    - protos
    - grit (interfaces, object model, serialization)
    - wit (intefaces, inner wit runner)
    - runtime (?, interfaces)
    - web
  - grit (grit server)
  - apex (orchestrator)
  - worker (runs actors)
  - inproc (in process runtime) (or "play", "reference", "inproc")
  - web (webserver)
  - cli (connects to apex and grit, or simple runtime)

