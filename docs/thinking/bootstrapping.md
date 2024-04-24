How can wits be bootstraped?

Wits need to be created, and know about each other.

Key instight: no runtime is needed for this, all bootstraping can be done by storing data in grid. The runtime should then pick up the agents.

(probably similar with updates, they can just be written (mostly) to grit and then picked up by the runtime and actors)
(and perhaps, bootsrapping and updating can be the same process: just syncing grit with a directory on the file system)
(nah, they should probably be different, otherwise the update might apply wrongly re-create new agents)

## Sync File

When loading wits into grid, there needs to be a way to tell where they live and what should be included.
We'll use TOML files (https://toml.io/en/v1.0.0) for that.

A discovery entry (or deployment entry) is a pointer to a wit directory, or file. Each entry is a TOML table.

```toml
[all] #optional
#push can also be a json (or inline table in TOML)
push = "path/to/common_code:/"
push_values = { 
    "/data/args": "hello world",
    "/data/more_args": {"hello": "world"},
}
push_on_genesis = "path/to/common_code:/"
pull = "path/to/common_code:/"
sync = "path/to/common_code:/" supports both push and pull

[[actors]]
name = "actor_one" #optional
push = "path/to/wit_one:/code" #is merged with all, can be ommitted, then the all sync is used
wit = "/code:module:function_name" 
wit_query = "/code:module:function_name" 

[[actors]]
name = "actor_two"
push = "path/to/wit_two" 
wit = "external:module:function_name" 
wit_query = "external:module:function_name" 
runtime = "python" #which runtime to use, default is python

```

## Mechanics

In grit refs, there is an entry that maps each actor name to the actor id. Through that, sync operations know which actor to update.

All push syncs happen by writing the appropriate genesis or update messages.

