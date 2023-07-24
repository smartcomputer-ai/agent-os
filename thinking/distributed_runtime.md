the runtime can be distributed. But there likley has to be a central runtime that organizes which actor runs where, since each actor should only be running once.
(but the running once requirement is not strict as long as outmessages are handled correctly, i.e. only outputs from one actor instance are routed.)

The central runtime has access to all kinds of user secrets. Distributed actors can ask for those secrets. This allows even actors that are runnint on the user machine to use all kinds of services directly.

Secret management should likley be separate from the refs and object_store.


Also, the best way to do remote client communication is running a wit on the remote client, instead of having the remote client trying to sync with a runtime/individual wit on the server
