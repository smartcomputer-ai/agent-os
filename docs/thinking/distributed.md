
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
  - inproc (in process runtime)
  - web (webserver)
  - cli (connects to apex and grit, or simple runtime)

NOTE: I have since changed the folder structure quite a bit.

## More thinking

Distributing the runtime (with separate Grit server and all), in a naive implementation, is about 100x slower! (100k object stores take about ~1s with in-proc lmdb, and 60s with the gRPC server running in separate processes)

Some of the optimizations: make the storing async, meaning the store call comes back immediately. But that gives us a consistency problem... because the worker can go down before the objects are persisted. However, the soluton is to buffer the writes, but only allow a head ref update once all the objects behind that step commit have been written. Also writes to the grit server could be batched (or gRCP streamed) to make less individual calls. I think all this could give us a 10 x speedup, resulting in 10msg per milliecond, which is likely good enough for now.

Also, one thing I thought of is to simply host an agent only in a single worker with a local store. But then the local store needs to be replicated... This could give much better performance, but then actors could not run on different workers unless we implement a more complicated replication structure.
