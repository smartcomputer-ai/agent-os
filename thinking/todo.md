
### Ground Zero

- [X] sleeps are needed for injecing messages to work in the runtime. there must be a bug
- [X] Test starting a runtime with pending messages
- [X] Make sure genesis messages is handled without any other messages, also that the next message arrives (create genesis msg and follow up msg and see if they are handled in two separate steps)
- [X] consider that the runtime should really be an actor too (with steps, and inbox and outbox, the core can be the runtime id.)
- [X] finalize refs naming scheme and add helper classes
- [X] support in-core wit functions (implement a custom python module loader)
   - [X] custom module loader to load from cores
   - [X] resolver that works with the custom core loader
   - [X] core resolver caching
- [X] Figure out how to do wit updates and implement it
   - [X] run update function (or default update function) on update message (one more branch besides genesis)
   - [X] add tests for updates
- [X] Filter genesis message if the actor already exists (if one gen message has already been handled), figure out the best way to indicate that the actor already exists (maybe just use the references store?) -> should be done inside the wit: if current_core_id == genesis_msg.core_id: return
- [X] Pydantic helper wrapers and classes for more strongly typed wit functions
   - [X] resolve blob messages into pydantic classes
   - [X] use wrapper classes to indicate which message type to match
- [X] Very basic chat agent that shows that "it works"


### Prepare for Friends

- [X] Cleanup file structure of project (where should everything go?)
   - [X] Introduce a 'patterns' folder from which wits can be constructed
- [X] file store is slow and not efficient, consider using sqlite or lmdb (lmdb is probably the best option, HOWEVER, sqlite is more ergonomic and known by devs)
- [X] cleanup and finalize the @wit helper funcions
- [ ] write a real agent that can do the following:
   - [ ] chat
   - [ ] write code and execute it (as a new actor)
   - [ ] ingest data and analyize it (embeddings, etc.)
   - [ ] generate some images
   - [ ] do it in a modular fashion (with funcation usage) that shows the power of the actor system and wits in general
- [X] Rename 'agent executor' to something else, it's just too confusing, maybe just call it 'runtime executor'
- [X] Add documentation to all relevant classes and functions
- [X] write some basic documentation on the architecture and how to use it
- [ ] Write a getting started guide
- [X] pick a name for the project: agent-os
- [X] move to different repo & apply MIT license


### Release v0.1

- [ ] install and use a linter (see: https://google.github.io/styleguide/pyguide.html)
- [ ] Add basic logging and instrumentation (instead of just prints)
- [ ] Review wit error handling
   - [ ] How should the first version work?
   - [ ] What is the end-goal of wit error semantics?
   - [ ] Document it.
- [ ] implement pruning / garbage collection v0.1 (can just be offline from the CLI)
- [ ] rename 'wit', because of wit.ai
- [ ] lockfile for the runtime and cli
- [ ] resolver cache clearing/invalidation, especially for CoreResolvers
- [ ] support sync wits (the objective is to support libraries like LangChain):
   - [X] sync alternative in object store
   - [X] sync version of data_model
   - [ ] allow sync wits, execute them in a thread
- [ ] consider renaming 'inbox' and 'outbox' to 'inputs' and 'outputs', makes it sound more neural-netty (I remember being somewhat put off by F#'s "MailboxProcessor".when I first saw it, which was really a simple actor model.) 'Mailbox' can become 'Channels'
- [ ] finalize url naming scheme for the web api. 
   - [ ] consider splitting up the webserver into wit and grit (get rid of the /ag/ root, it's ugly, probbaly go back to /wit and /grit root paths)
   - [ ] remove the 'web' paths from the web server (should be served by queries instead) (consider supporting tree descent in /grit/objects)
   - [ ] add support for a default foward path for an agent (e.g., go to /ag/test/web and then be fowarded to ag/test/wit/agents/frontent/web)
- [ ] what are the requirements for the first version documentation?
- [ ] automatic LMDB resizing
- [ ] automatic pruning of object store
   - [ ] define "sleep" semantics for step chains and message chains (inbox and outbox), so that actors can prepare for pruning
- [ ] implement cancelation of wit execution if all messages are signals (50% done already)

