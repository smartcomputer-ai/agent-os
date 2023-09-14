# Rails

Rails allow the orchestration of multiple wit functions *synchronously*. Rails use utility actors under the hood that proxy messages.

## Problem
Since wit functions are fundamentally async, it is hard to compose multiple wit functions together.

Let's say we have a wit that downloads an image and saves it in grit

```
async def download_image(str url, store, outbox)
    img = requests.get(url)
    img_id = store.store(img)
    oubox.send(requester_id, img_id)
```

How should we reasonably call this function from a different wit?

Enter continuations or completion handlers. This is how async was done back in the day: with callbacks. The problem with callbacks is that they are hard to compose. You can't just call a function and get a result back. You have to pass in a function that will be called when the result is ready.

In the case of Wit message handlers, what needs to be correct is only the returning message type and maybe some sort of correlation id to be able to assoiate requests with responses (or, what is the same, commands with events).

## IO Monad
One way to solve this, especially if multiple actors need to be coordinated is to use a monad. For example, Urbit, which is entirely async in its "agents", uses "treads" to coordinate async actions. In Urbit, a thread is a monad that can be used to compose async actions. https://developers.urbit.org/reference/arvo/threads/overview

## Introducing "Rails" 
Let's call our IO monad "rails" (or "trains", not sure yet). A rail defines a linear path of several chained Wit function calls. Specificaly, it enables request-response patterns with wit functions, but also other things like timeouts, and so on.

In the agent OS, rails can only be properly started from the runtime itself (or the actor executor), a wit can use a rail helper, which is passed via context, to start a rail. Under the hood, a rail is just a wit too that proxies events for the caller.

## Deadlocks
As long as the rails-subsystem does not allow reentracy into the actor that "owns" or initiated the rail, dead-locks can be avoided. Also, as long as a rail is active, no other actor should be allowed to create a different rail that messages an actor with that active rail. Again, this could cause deadlocks.

## Workers and Coordination
There should probably be a "rails worker" that runs on the same worker as the actor runs, and consequently there might be many of them. And a main rails wit, that cooridnates all existing rails (probably with a timeout). Or, at least, the rails coorinator needs to kick in if a particular rail references actors that are not managed on the local worker.

## Perf
Rails will be sloooow! Because it will require the compution of many steps, both the actual composed steps of real wits, and internally to store the state. So they should be used with caution.