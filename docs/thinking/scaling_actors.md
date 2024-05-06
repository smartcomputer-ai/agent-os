# How to Scale Individual Actors?

Idea: allow a wit to mark itself as "parallelizable" and then the runtime can create multiple instances of that actor. Instead of there being a single head ref, there will be multiple, managed by the runtime to distribute load. So bascially an actor that will be instantiated multipe times.

A thing to consider, to keep the state management more predictable, is to not allow those kinds of actors to modify their own core. Ie. they are only allowed to keep their original core that defined their id, maybe with all cores being the same?

This the runtime could then round-robin incoming messages to those scaled actors. And it could even propose the messages in some order to each actor, and the actor could choose to take it from the inbox or not. Such a setup would allow actors to work as parallelized consumers in a kind of producer-consumer pattern. 

This kind of scaled setup is also desireable for the root actor which will see a lot of traffic.

Moreover, if we implement channels in the messages, this would make it even more powerful. The runtime could then route messages to the correct actor based on the channel. This would allow for a more fine-grained control of ehich scaled actors gets or takes a message.

The only downside is that this would result in multiple inboxes and outboxes with differeent steps. However, if the scaled actors share a common actor id, this is not so much a problem because to other, singleton actors, the scaled actor looks like a single, addressable actor--and a single sender as well.

With messaging there probably needs to be some more rules so that this works, especially with sending messages from scaled actors that are queues. If multiple actors produce a queue and send it to the same singleton actor, then there is an ambiguity on which queue to accept from all those senders. The solution is probably to either have different actor ids for each scaled actor (but then we lose other benefits), have scaled actors only send signals, or have them always use channels for sending messages that indentify the sending actor is some other way. The final option is to have scaled actors be addressable either by a shared actor id or by an individual id that is different for each scaled actor. The sender id from scaled actors would always be their individual id.