# How to Prune (GC) Grit?

How do we prune the Grit DAG? This requires a garbage collection type system.

The main idea is pretty simple: grit needs to be extended so that the "previous" fields in messages or steps can be set to "None" while maintaining a "soft-link" to the history.

Here is how this could look like:
```
Message = NamedTuple("Message", 
    [('previous', MessageId | None),
     ('previous-pruned', MessageId | None), --only one or the other "previous.." would be allowed to be set
     ('headers', Headers | None),
     ('type', str),
     ('content', BlobId | TreeId | None)])
```

If `previous-pruned` is set, `previous` is not allowed to be set. Although this maintains a historical link to the obj ids that came before, they can be discarded by the garbage collector.

## Messages
Maintaining a link to the history is important for the message object type because it allows an actor to send a pruned message list, giving the recipient a chane to process also previous, now pruned messages before it accepts the message with the prune marker.

## Lifecycle
The runtime would probably send "prune" signals to each actor when it is time to prune. But actors could also decide to prune messages or step histories on their own initiative. The mechanics would be the same

 1) Runtime sends "prune signal" via normal message
 2) Actor sends a pruned message to all or most of its outbox
 3) Actor also incorporates a prune marker in the new step
 5) (later and indepenently) reviever accepts the pruned messages in its inbox, completeing the cycle for that message channel.
 4) Grit can now garbage collect the messages and steps that are not needed anymore

## Maintaining Some History

It would be nice if an actor could retain *some* history of what happened to it. That is, if a prune request does not prune all the way to the present moment, but rather a little bit back. 

How much back could be configurable (or part of the prune request signal).

How to do this?
```
Message = NamedTuple("Message", 
    [('previous', MessageId | None),
     ('prune-from', MessageId | None), --if set, prunes back from the message id specified here
     ('headers', Headers | None),
     ('type', str),
     ('content', BlobId | TreeId | None)])
```
In this case, `previous` would still be always set (if it is a message queue), and `prune-from` would indicate any message id in the history of previous messages where to prune from...

However, it is not certain if this is the best design. It requires a lot on the part of the actor. Altenatively, the pruning happens often, which would result in many pruning markers throughout the history, *and then the runtime or Grit decides what to actually prune.*

In the second design, the prune messages could also contain some sort of timestamp which allows grit to decide, but grit could maintain that timestamp too.

With the most sleek design the message could just have a flag whether pruning previous messages is allowed, everything else would stay the same:

```
Message = NamedTuple("Message", 
    [('previous', MessageId | None),
     ('prune', bool), --if set, prunes back from the message id specified here
     ('headers', Headers | None),
     ('type', str),
     ('content', BlobId | TreeId | None)])
```

I'm not sure if there is a better way to indicate the prune marker... I think somehow the first design at the very top is better, because it makes the prune action much more explicit than a flag (branching mechanisms have to be introduced anyhow).

Finally, it could also be that we simply have Grit track the time and prune without any markers and/or involvement of the actors. But that would make it difficult to guess whether data or history is available or not. Especially if the wit logic relies on the history (such as comparing two obejects how they changed over history). The actor would have no way to know why data is not available in Grit (although we could return a "pruned" object if it doesnt exist anymore, but then that would work like an additional null, which is bad, better make it explicit).
