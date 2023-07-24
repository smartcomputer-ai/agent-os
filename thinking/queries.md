# Wit Queries
Besides the wit state-transition function, we can define also another function that does not (really) produce side effects (technically it does since it queries grit, which uses IO, but it does not change grit).

We are calling these functions queries, because it allows us to query the state of a with. Or more specifically it allows us to query a certain step that contains a wit.

## Definition

The most simple definition would look like this:
```
def wit_query(step) -> any:
```

The query function has access to grit, so it can read all the data that is inside the `step` object and its children (core, inbox, outbox, previous steps, etc).

But let's do a bit better and define a universal interface for reading operations.
```
def wit_query(step:StepId, query_name:str, context:Blob) -> Blob | Tree | Error:
```

Note, althought this is using Python (pseudo-)code, this is implementation works in any programming language.

A query does not write any data, it just retrieves data from the step, and uses the context, optionally, to return a result of arbitrary data. It does not strictly have to do anything with the step. It can be used to serve HTML pages, or a made up image, or whatever.

It takes the following inputs:
- `step` is the step id of the latest step (on how this is step id is picked, see below). The function can then use the object_store to retrieve all the data in the core.
- `query_name` is the name of the query that should be executed. This allows us to define multiple queries for a wit.
- `context` is a blob that contains the context for the query. This can be used to pass parameters to the query, or larger data for more heavy duty processing.

Finally, the query returns a blob or a tree, or an error. It does not return the id of these objects, but the actual data, which is encoded in the standard object serilaization format that is used in the object store. This is because these results are *not* persisted in the object store when the query executes.

A tree is used to return pointers to existing object_ids in the the step's core or mailboxes. This is useful if the query wants to return a list of object_ids, or if the query wants to return a single object_id that is already in the core or mailboxes. If the the query returns novel data, it should return a blob.

If the query is not supported, or does not exist, the result should be an error that indicates this. Errors can also be returned if the query fails for other reasons.

Queries are defined in the core of the actor, just like the wit state-transition function itself.

## Conventions
To make the use of this query interface a mit more usable, we can define two conventions. One for discover, and one for arguments.

A wit *may* define a query called `wut`, which returns a JSON blob that defines what other queries it supports, what wit input messages it accepts, and what output messages it accepts. Plus, it may include natural language that explains what any of these do (to make it easier for LLM tool usage).

Secondly, most wit queries should accept a JSON blob as context that is strucutred as as key/value argument list. This list can then be expanded into normal function calls by the runtime to make query authoring more ergonomic.

## Calling a query

Any wit, or permissioned outside system, can call queries of known actor ids. Normally they either know which queries are supported, or they can call the `wut` query to find out.

When a different wit issues a query it does not know the step_id. So this is usually supplied by the runtime. The caller just calls like this:
```
results = query_a_wit(actor_id, query_name, context)
```

## Runtime Implementation

Queries can run completely independently. They just look at the step HEAD and then run the query.

The runtime also provides a web API to execute these queries.

