# Updating Actors

Updates are tricky.

Here is my current idea:
 1. An update is just a message with a new core, and a header `{"mt": "update"}`.
 2. The wit is then executes *as usual*, that is, the wit from the previous step is ran to execute the transition function. The wit should not do special update work, but should perform any internal state cleanup in preparation for the update. Most of the time, the wit ignores the message thoug.
 3. The runtime then does treats the message like a genesis message: it looks for a `wit_update` node in the core. If it finds one, it runs the one *in* the core of the update. The `wit_update` node is a function that takes the old core and returns a new core, usually in the process replacing it, and migrating any state.

 In effect, update messages, run two state transition right one after another.

## Wire it Up
The `actor_executor` (and/or runtime) will do increasingly more work to ensure that steps get executed correctly (and that safety precausions are respected). The ideal place to put an update function is in `_resolve_wit_function`, where the executor can detect if there is an update message and then *wrap* the wit function inside an update function. It would then work in the following way: 
 1. The wit computes its next step but does not merge the core.
 2. If thre is a `wit_update` entry in the *new* core, it executes it. 
 3. If there is no update function it just uses a default one that merges the cores relatively naively.