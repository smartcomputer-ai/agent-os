# Agent OS: Design and Architecture

Here, we're going to take an in-depth look at the technical design principles and architecture of the Agent OS. If you want to understand how everything works and fits together, then this is the place to start. You can think of it as a whitepaper. It is a relatively long document, and you can hop around as you wish, although the knowledge conveyed here does build on top of each other as you read through each section.

What this document doesn't convey is how to build autonomous agents. That is for a different tome. However, we believe that it is extremely important how we structure the compute environment of where AI agents will live. In many ways, the *substrate* where agents are instantiated will define their nature. Of course, it is foolish to build infrastructure without any idea what it will be used for. We do have certain convictions about how agents should work, and they are mirrored in the design. 

Why is it called an "operating system"? The Agent OS is not meant to be installed like a traditional OS, such as Linux or Windows. Instead, think of it as an execution environment for agents. When Amazon started AWS, they thought of AWS as a "cloud operating system". By OS they meant a set of APIs and subsystems that made cloud infrastructure tractable. The Agent OS aims to provide a substrate that makes writing powerful and long-lived autonomous agents tractable.


## Desired Properties
Before we can describe how the Agent OS works, we need to describe the properties we want it to have.

  1) The goal is to build a personal agent that you can trust that it is working *for you* and not someone else. You need to be certain that an agent only acts on your goals and intentions and not those of some other party or company. The architecture needs to consider this desire as foundational, even if it is in tension with other desired properties.
  2) The more context an agent has, the better it can help you. It is important that an agent has access to as much user data as possible. At a minimum, it needs the data relevant to the goals of your agent. Therefore, the system needs to be able to ingest and process a lot of information. Ideally, you should be able to pipe your entire digital life into your agent without any worries.
  3) However, personal data needs to be kept private. And the user needs to have the option to protect certain information and be able to trust that the system doesn't leak it to unwelcome third parties.
  4) Moreover, a personal agent is no good if it cannot be your companion for a long time. If the agent must be reset after each task, it is no real personal agent. Consequently, the system needs to be designed for longevity.
  5) Agents need to be able to *do* stuff; they need to be able to take meaningful action in your name. The way an agent is going to do that is by writing its own code (using LLMs) and then executing it. As a consequence, the code base of each agent will diverge from the code of other agents. In many ways, each agent will be unique. This is a major departure from traditional software development and deployment methodologies where the code is usually the same for all users and only the data changes. In the Agent OS, it should be trivial to spin up a new function, wire it up to the relevant data, and execute it, while trusting that the code is properly sandboxed and does not jeopardize the integrity of the agent.
  6) Additionally, agents should be able to act in parallel, not just sequentially. Just like in the internals of a human being, there are many things that are happening at the same time, some of them conscious, others not. The OS should ensure that the agent has the resources and ability to execute different thought processes (LLM completions) and other programs at the same time. And do it as real-time as possible.
  7) Finally, agents should be running all the time, reacting to incoming events, even when the user sleeps. Currently, most generative AI tools are prompt-to-response, and that's it. Conversing with the agent via natural language is just one modality of the agent learning something or starting a process. There will be many more. For example, an email arriving, the agent finding some new information on the internet while it’s running a nightly research program, a timer or calendar event firing, and so on.

These properties constrain our design boundaries considerably, but we believe that the architecture described here can realize these goals.


## High Level Design
Agents do not have to run all in one place or on one machine. Parts could run in the cloud, and other parts on your personal machine or mobile phone. Agents should be able to marshal whatever compute resources they need to achieve their goals. The important thing is that agents are a unified *conceptually*: a user needs to be able to delineate what constitutes *their* agent and what doesn't. As long as that's guaranteed, the agent can be an extremely dynamic system. Consequently, the Agent OS can be thought of as the orchestration layer, or runtime, that makes this possible.

The architecture set forth here combines a strongly consistent data layer with a relatively simple [actor model](https://en.wikipedia.org/wiki/Actor_model). Think of actors as "sub-components" or "objects" that encapsulate a certain functionality. The persistence layer acts as a file system of sorts. Actors pass messages to each other utilizing the data layer. While individual actors are single threaded, other actors can run concurrently. Because we want actors to write their own code, most actors execute their code not from a traditional file system but right out of the data layer. Accordingly, the persistence layer contains both the data and the code of each actor. There is also a runtime that coordinates the execution of the actors and ensures the integrity of the data layer.

For those familiar with technical jargon: the Agent OS follows a [virtual](https://www.microsoft.com/en-us/research/publication/orleans-distributed-virtual-actors-for-programmability-and-scalability/) actor model architecture, with [event driven messaging](https://en.wikipedia.org/wiki/Event-driven_architecture) (but state is *not* event-sourced, at least not strictly), and is implemented as kind of self-executing Git alternative. The data layer is an append-only [Merkle](https://en.wikipedia.org/wiki/Merkle_tree) [DAG](https://docs.ipfs.tech/concepts/merkle-dag), which gets periodically pruned, and which functions as a [content-addressable storage system](https://en.wikipedia.org/wiki/Content-addressable_storage). Since both code and data live side-by-side, and actors operate mostly on the data layer, we can think of the system as a poor man's [single-level store](https://en.wikipedia.org/wiki/Single-level_store). Moreover, since the all actors operate on the same data store, it also functions as a [global namespace](https://en.wikipedia.org/wiki/Global_file_system) in which data can be referenced and accessed freely via hash ids (SHA-256), allowing actors to build extensive data graphs and knowledge bases.

In the Agent OS, there are the following sub-systems or components:
 - **Grit**: the data persistence layer, which borrows liberally from the design of Git, but operates more like a database and file system
 - **Wit**: the state transition function of an actor, which takes as its input its current state and new messages, and then produces a new state and output messages
 - **Runtime**: the orchestrator that makes sure the Wit function of an actor executes properly and that messages are routed between actors. It also controls access to Grit

<p><img src="agent-runtime-grit-actors.png" alt="Architecture Overview" width="600" />
Figure 1: An agent consists of the Runtime, Grit, and several actors. Each actor consists of its state transition function (Wit) and its state, both of which are stored in Grit itself. The runtime, here, is executing "Actor B" by passing new input messages to its function "Wit b". As part of the execution, the Wit also produces new output messages, which are then routed to other actors. Also, during execution the actor might change its internal state.
</p>

One of the explicit design goals is that the execution and persistence models are simple enough that different programming languages can be trivially supported. It should take only 1000-2000 lines of code to implement an executor that can host actors written in a particular programming language. (The runtime is a bit more complicated, but the runtime does not have to be re-implemented for each programming language). Since we want to be able to run parts of an agent on different platforms—e.g., local machine, mobile, cloud, browser—this will come in handy.

It should also be noted that there is nothing AI or ML related built into the base primitives of the Agent OS itself. Nor is it designed to train models or run inference on models, though the latter is certainly possible. The OS treats models as external "peripherals." The system could also be used to build different types of applications, unrelated to AI and agents. However, the architecture is explicitly designed for agents and any tradeoffs are tilted towards fulfilling the goals mentioned above. Specifically, the technical goal is being able to utilize the user's data while prompting LLMs, build an execution environment that can run code generated by LLMs, and run autonomously, even when the user is not prompting the agent.


## Python Prototype
Right now, the entire Agent OS is written in Python. But in its current form, it's just a proof of concept—albeit a serious one. The goal is to rapidly validate the design and architecture of the Agent OS and then implement the runtime in a systems programming language like Rust or Go. Of course, the actors themselves will continue to be written in a high-level programming language like Python.

Therefore, the current Python code focuses on finalizing the actor API and showcasing some of the use-cases in designing autonomous agents based on the Agent OS.

To make it a bit more concrete, here is how the code of a simple actor looks that implements a GPT chatbot.
```Python
from grit import *
from wit import *
from common import *

#the app is the entry point for the actor
app = Wit()
#usa decorator to add message handler to the wit app
@app.message("new_user_message")
async def on_new_user_message(messages_tree:TreeObject, ctx:MessageContext) -> None:
    # use the message tree, which is a Grit object,
    # to load all the historical messages
    messages = await ChatMessage.load_from_tree(messages_tree)
    if len(messages) == 0 :
        return
    # ensure that the last message was from the user
    last_message = messages[-1]
    if last_message.from_name != 'user':
        return
    # call out to the Open AI API for a chat completion
    new_chat_message = await chat_completion(messages, actor_id=ctx.actor_id)
    # send a reply message to the frontend
    ctx.outbox.add(OutboxMessage.from_reply(
        ctx.message, 
        BlobObject.from_json(new_chat_message), 
        mt="head_reply"))
```
Much of this won't be fully understood yet, but here are a few things to point out: `load_from_tree`, `chat_completion`, and the function itself are async. We make extensive use of asynchronous programming to run many actors in the same process. The code uses a function decorator to wire it up to the runtime and quite a bit is going on under the hood to make this work. We will get into how all of this works below, but the takeaway should be that it is relatively simple to write an actor.


## Actors
An agent consists of one or more "actors." So, the "agent" is the whole thing, and an "actor" is just a sub-component or part of the agent. But conversely, since Grit, the persistence layer, can only be modified by an actor, an agent is just the sum of all its actors. In the Agent OS, it's [actors all the way down](https://en.wikipedia.org/wiki/Society_of_Mind).

Generally, when talking about actor models, the key insight is that actors run in parallel, but each actor runs in a single thread or process, which makes it thread safe. Now, in our case, we are using a virtual actor model and execute them in an asynchronous message loop. Virtual means an actor doesn't have to be instantiated all the time, only when it needs to compute something, and it is safe to try to create it multiple times. Asynchronous means that actors do cooperative multitasking, relinquishing control of the thread or process whenever they do not need it. The upside is that an agent can easily consist of thousands of actors, even if it runs on a user's laptop.

An actor does not need to do any data locking internally. This is because actors communicate by message passing: if an actor wants information from another actor, it has to be done via message. They never share mutable memory or state. For example, actor A sends a "request for X" message to actor B. Actor B then sends a "response with X" message to actor A. That's all there is to the actor model. Implementing this model is mostly about creating a runtime that executes the actors in parallel and ensure that messages are delivered to the correct actor.

At the heart of our actor model there is the "Wit." The Wit is the state transition function that accepts a message and applies it to a state variable in order to create a new state. The exact definition of a Wit function is given below after going into the details of Grit, because that's a prerequisite. But for now, what you need to know is that an actor consists of its internal state and a Wit function. It accepts input messages from other actors, modifies its internal state, and produces output messages which address other actors. Finally, a Wit is not an actor, because the same Wit can be used in different actors; an actor is Wit+state.

What is unique about our actor model is that it is built right into our persistence layer, Grit. Many other actor implementations are persistence agnostic. In the Agent OS, messages are just persisted `message` objects, and an agent's state is the latest saved `step` object. More on that shortly.

Moreover, all actors share an immutable, global namespace, meaning there is a common data layer that all agents can safely share. If an agent has a reference to the right object identifier from Grit, they can share existing data through those object ids (the data is still shared via a message, but the message only contains a reference to an object id). Orderly execution and safe data access is guaranteed because the identifiers are hashes of the data, and consequently agent B can only know of data X after agent A has created it (due to the nature of hashes), and X is immutable once it has been created.

To understand how this works, let's look first at how Grit defines an immutable, yet extensible, object model and data structure. The Grit object model is largely inspired by Git, but with some key differences to make it work with message passing.


## Grit: Object Store
Let's first consider Git. Git has two very nice properties: it has a very clean and simple state transition function modeled as "commits," and its internal data structure is a Merkle-DAG. We don't use Git itself, but we borrow some of the key ideas from it. We call our Git variant "Grit." 

One way to look at Git is to think of it as a state transition function that takes in the previous state of the Git repository, combines that with new file diffs from your filesystem, and then produces a new commit. 

Further, Git is an append-only Merkle-DAG. Simply put, this means that all data is in a tree structure where each node is identified by a hash which is made up of all the hashes of its child nodes. And it's a directed graph because commits can reference other commits. It's append-only because new files, or changed files, are always appended to the object store by computing a new object id that is the hash of the contents. Objects are never updated. Unused files or objects can be garbage collected later.

The Agent OS has a similar serialization format as Git and also has an object and reference store, but we renamed "commits" to "steps" and added two more object types, called "messages" and "inboxes." 

What is useful about such types of data structures is that they are super easy to reason about and very simple to implement. The first version of Git was 1000 lines of C. For us, the ability to reason about Grit is crucial because implementing asynchronous, concurrent actors is already a challenge. If the data model is super simple and fool-proof, that gives as a good foundation to build on. Also, it being simple to implement is important because we want different programming languages to support Grit (specifically, the serialization format). Finally, it is also very easy to analyze the DAG and figure out what data is used and what isn't, and ensure the longevity of the first, and garbage collect the latter.

Let's learn how the actual data structures work.

### Git Data Structure
Here is the pseudo code for how Git works. Notice that it's basically just three structures: blobs, trees, and commits.
```
type blob =  array<byte>
type tree = dict<string, tree | blob> 
type commit = struct {
    parents: array<commit>
    snapshot: tree
    author: string
    message: string
}

type object = blob | tree | commit

objects = dict<string, object>

def store(object)
    id = sha1(object)
    objects[id] = object
    return id

def load(id)
    return objects[id]

reference = dict<string, string> <- name to sha1
```
It's incredibly elegant, especially considering how ubiquitous Git has become. The store just takes an object, computes its hash, and saves it in some sort of key-value store (here, an in-memory map or dictionary). To know which commit is the latest and to create branches, Git also utilizes a “reference store”, which maps human readable strings to hashes. For example, the 'main' branch is '5f0380e..7c7a95e'.

In actuality, none of the references to other objects in the tree or commit are the actual objects. Instead, they contain the SHA-1 hash of the respective object. For example, the tree object contains the hash of the blobs or trees it contains. The commit object contains the hash of the tree object it contains (`snapshot`). And so it looks more like this:

```
objectId = str
blobId = objectId
treeId = objectId
commitId = objectId

type blob =  array<byte>
type tree = map<string, treeId | blobId>
type commit = struct {
    parents: array<commitId>
    snapshot: treeId
    author: string
    message: string
}
#...etc
```

Now, how to use this structure? If you want to change a file, you create a new blob object with the new file contents, and then create a new tree object with the new blob object, and then create a new commit object with the new tree object. The old objects are still in the store, but they are no longer referenced by the new commit (although the new commit does reference the previous commit, and so the whole structure is technically a DAG that contains the whole history). This is how Merkle trees work, if any node changes the hashes of all the parent nodes must be re-computed, all the way to the root. In practice, this overhead is quite manageable though.

If it's still not clear, here is a good [explanation of how Git works](https://codewords.recurse.com/issues/two/git-from-the-inside-out).

### Grit Data Structure
The way Grit works is largely inspired by Git. However, instead of having `commits` we have `steps`. And there are two new objects: `message` and `mailbox`

Besides the new object types, the biggest difference to Git is that each Grit "repository" can have many parallel steps with independent sub-trees. A Git repository manages only a single directory tree. Or another way to look at it: Git has only one `commit` HEAD; Grit has many `step` HEADS. Think of a Grit namespace as consisting of many different repositories that contain unrelated data but may reference each other.

Here is the [actual Python code](/src/grit/object_model.py) that defines the entire Grit data model:
```Python
ObjectId = bytes # 32 bytes, sha256 of object

BlobId = ObjectId
Headers = dict[str, str]
Blob = NamedTuple("Blob",
    [('headers', Headers | None),
     ('data', bytes)])

TreeId = ObjectId
Tree = dict[str, BlobId | TreeId] # key must follow Unix file name rules

MessageId = ObjectId
Message = NamedTuple("Message", 
    [('previous', MessageId | None), # if None, it's a signal, otherwise, a queue
     ('headers', Headers | None),
     ('content', BlobId | TreeId)])
MailboxId = ObjectId
ActorId = ObjectId # hash of core of message that created the actor, i.e, object id of the core tree
Mailbox = dict[ActorId, MessageId]

StepId = ObjectId
Step = NamedTuple("Step",
    [('previous', StepId | None),
     ('actor', ActorId),
     ('inbox', MailboxId | None),
     ('outbox', MailboxId | None),
     ('core', TreeId)])

Object = Blob | Tree | Message | Mailbox | Step
```
And here is the pseudo code for the Grit object store. The actual interfaces can be found [here](/src/grit/object_store.py).
```Python
objects = dict<string, object>

def store(object)
    id = sha256(object)
    objects[id] = object
    return id

def load(id)
    return objects[id]

reference = dict<string, string> # name to sha256
```

Let's look at each object type in Grit.

#### Blob & Tree
In the Grit object model, `blob`s and `tree`s are basically the same as in Git, except that blobs can have headers and in Git they don't.

All actual contents, of all data, is stored in the end as a blob. Trees are just a way to organize blobs; just like we use directories in a normal file system. However, unlike a traditional file system, it is not possible to create an empty tree. A tree must either contain more trees or at least one blob object.


#### Message & Mailbox
A `message` points to a blob or tree as its actual contents. It can also have `headers`, which are sometimes utilized by the runtime or actors for message dispatching. A message object can function like a linked list if it has a reference to a `previous` message. We utilize this to build message queues. If there is no previous message id, we consider a message to be a signal, and signals can override each other in rapid succession, interrupting the current execution of the actor. Queues, on the other hand, should be processed in order. 

Since messages don't carry any sender or recipient information, we need one more data structure. The `mailbox` dictionary can be used to either contain pairs of `(sender_id, message_id)` or pairs of `(recipient_id, message_id)`, where the senders or recipients refer to other actors. Mailboxes are used in the `step` object if you look at the `inbox` and `outbox` fields.

#### Step
`step` objects are the result of running a Wit state transition function, i.e., they are the output of a Wit. Each time an agent is run—which means its Wit is run with new messages and its current state—a new step gets created.

A step must reference a `core`, which is just a tree that points to the root of the step's internal state, both code and data. The runtime expects a core to be of a certain shape, or rather, contain a few items structured in a certain way—mostly a `/wit` node that defines the entry point for the state transition function and some other details, all of which we will investigate below. Other than that, the core can contain anything, possibly terabytes of data. 

Tangentially, an actor is identified by the hash of the *first* core of the *first* step. This id remains the same for the lifetime of the actor, even as the hash of the core changes as the actor updates its state. Remember, an actor is Wit+state, and so an actor's id is `hash(initial state+initial wit)`, or equivalently: `hash(initial core)`. This is an important detail for the virtual actor model considered further on.

The `outbox` field of a new step contains all the messages that the actor is sending to other actors. It's a mapping of recipient actor ids to message ids. It's optional for a step to produce or update an outbox. If the outbox has contents, the runtime compares the outbox from the previous step with the outbox of the new step to see if there are any new messages to be sent to other actors, and if so, routes them.

A step's `inbox` contain *the messages that the actor has read so far*. So, it's more a "read inbox". It's a mapping of sender actor ids to message ids. It does not contain newly routed messages from other outboxes. New messages are proposed by the runtime separately when it executes the Wit function. The actor then decides to "accept" or "process" a message by making it part of its inbox when it generates a new step. If a message is a linked list of messages, the inbox can be a "cursor" of up to where the agent has read the inbox. This allows a Wit to implement single message processing or batch processing. Also note that message ordering is only guaranteed from an individual sender, not from all senders across, because each sender has its own queue (but only if the sender links the messages using `message.previous`).

The `step.previous` property creates a linked list, or "event log," of all the steps since the beginning of an actor. In reality, though, that link needs to be broken sometimes so that old data can get pruned. Unlike Git, where the `parents` field of a commit is immensely important for the history of a repository (commits, branches, and merges), it fulfils a more transitory function in Grit (for example, if there are accidental multiple parallel executions of the same agent of the same step). 

Since `step.previous` is not an array, steps do not permit merges. And it makes little sense to merge two divergent step chains because in the Agent OS it's easy to fork an actor by creating a brand new step history and then copy/merge any data later, without merging the steps themselves.

So, to recap, any given step object contains the entire state and history of an actor:
 - history: `step.pervious` and `step.actor` (initial core)
 - internal state: `step.core`
 - received and read messages: `step.inbox`
 - sent messages: `step.outbox`

### References
In Grit, there is also a "reference store." It [consists](/src/grit/references.py) of two simple functions: 
```Python
get_ref(ref:str) -> ObjectId
set_ref(ref:str, object_id:ObjectId) -> None
```
References are utilized by the runtime to know the state of affairs of the entire Grit namespace. Specifically, there are a few conventions that need to be observed to keep the system as a whole consistent.

Actors themselves are not allowed to access the reference store, because it could lead to deadlocks, because actors could use the reference store as a second communication or signal channel. Actors must only communicate via messages. Consequently, only the runtime itself is allowed to use or change references.

#### Actor Head Refs
An actor exists if it has a "head" entry in `/refs/heads/<actor id>`. The actor id is a hex string, and the reference points to the *latest* step id of that actor.

So, the Agent OS, as a whole, is constituted by all the head references of all its actors. If there are, say, 20 actors that make up an agent, we expect there to be 20 head references that point to 20 unique step chains. This is a major difference to Git: although a Git repository can have many branches, it has only one head reference. 

The head reference gets updated by the runtime every time a new step id is computed in a state transition function.

#### Actor Name Refs
For convenience, an actor can have an alias, which is a normal string. This alias or name must point to the actor’s id. The purpose is to make it easier to develop actors because an engineer can pinpoint a specific actor by name. Internally, though, when actors communicate with each other, they must use actor ids instead of names (because the mailbox object expects actor ids).

### Grit Summary
Grit is an append-only key-value store that uses the hashes of its values as keys, and it only supports five types of values or objects: blob, tree, message, mailbox, and step. Anything more complicated has to be stowed away in a blob and handled on the application layer.

If you squint at it, especially the progression of steps, you can see that Grit is a kind of filesystem, Git-like source control, and database all in one.

The goal is to keep Grit so simple that it will rarely change, if ever. Basically the [*serialization format* of Grit](/src/grit/object_serialization.py) is the primary protocol of how components talk to each other in the Agent OS. And as long as components implement that protocol, they can play and operate in the OS.

The object model is structured in such a way that it allows the implementation of an actor model on top of Grit, although Grit itself does not actually execute or run any actor. A runtime is required to make the OS move. But first let's learn how the Wit state transition function generates steps.


## Wit: State Transition Function
An agent consists of several actors, and each actor is defined by its state transition function. A state transition function is a procedure that takes in `current state + new data` and computes `new state`. We call this function the "Wit" of an actor. The inputs and outputs of this function are exactly defined in terms of Grit objects. 

*Note: our "Wit" functions have nothing to do with [wit.ai](https://wit.ai), the NLP platform by Meta for chatbots. The naming clash is just an unfortunate coincidence. In the worst case, it will force us to rename our concept of strongly persistent actors with a fixed state transition function to something different.*
 
### Creating Steps
Here is the definition of the Wit function:
```
current step + new messages -> new step
```
Or using the Grit object model definitions from above:
```Python
def wit(last_step_id:StepId, new_messages:Mailbox) -> StepId
```
A Wit takes in the last step id, and then "applies" the new messages to its internal state and in doing so produces a new step id. The process usually consists in modifying its core, but minimally it updates the inbox of the step to mark the new messages as read.

You might have noticed that there is a problem: how to get from last step *id* to new *id*? Clearly, more is required than what is provided in the function inputs to generate a new step id. This is especially the case if you expected the transition function to be deterministic and side-effect free. However, in the Agent OS, *Wit functions can have side effects*! So, technically speaking, the function definition as it stands is correct, because the function implementer can call other APIs or libraries to generate the new step. Now, in practice, since the data store and other services are managed by the runtime, these dependencies are injected into the Wit function, giving us the following function:
```
current step + new messages + object store + other dependencies -> new step
```
Or, again, in Python:
```Python
def wit(last_step_id:StepId, new_messages:Mailbox, **kwargs) -> StepId
#or
def wit(last_step_id:StepId, new_messages:Mailbox, object_store:ObjectStore, **kwargs) -> StepId
```
Where the `kwargs` (which is a dictionary) contains things like the object store and information about the environment (in other similar systems this is usually called the "context").

When an agent's Wit produces a new step, all the runtime is concerned with is the internal consistency of Grit and updating the Grit references to the latest step id (i.e. set `refs/heads/<agent id>` to `<new step id>`). Besides that, the Wit can do whatever it wants. It can utilize external resources, such as services, files, databases, ML models, and APIs, at will. The only thing to consider when doing this is that the execution of a Wit can fail or be canceled by the runtime and retried later, so it is the developer's job to make external write operations idempotent or implement other safeguards.

### Low-Level API
The basic low-level API basically consists entirely of using the object store to load and persist Grit objects.

It is expected that developers will rarely implement a Wit function in its raw definition, because it will require too much boilerplate code to read the state of the old step, modify the state, and then generate the new step id. 

Nonetheless, here is how a Wit function will look like if it just utilizes the object store and no higher-level library. It is long but it is helpful to see at least once how Wit would be implemented manually.
```Python
# a wit that saves greetings to its own core
async def wit(last_step_id:StepId, new_messages:Mailbox, store:ObjectStore, **kwargs) -> StepId:
    # read last step
    step = await store.load(last_step_id)
    inbox = await store.load(step.inbox)
    outbox = await store.load(step.outbox)
    core = await store.load(step.core)
    # assume the core already has a node called 'my_greetings'
    my_greetings:Tree = await store.load(core["my_greetings"])
    # read new messages
    for sender_id, message_id in new_messages.items():
        # see if the message has been processed before.
        # however, checking the inbox like this is not sufficient,
        # the entire message chain (using `previous`) has to be checked
        # but for now, let's keep the example more simple
        if sender_id not in inbox or inbox[sender_id] != message_id:
            #set the message_id as read
            inbox[sender_id] = message_id
            message = await store.load(message_id)
            # having a head called 'mt' is a convention that stands 
            # for 'message type'
            if(message.headers["mt"] == "greeting")
                # assume the message contains a blob of string contents
                content:Blob = await store.load(message.content)
                # print the contents for debugging
                # remember, blobs consist just of bytes, so a decode 
                # is required
                print("new message contents", content.data.decode("utf-8"))
                # use the current datetime for the node key
                greeting_key = "{date:%Y-%m-%d_%H:%M:%S}.txt".format(date=datetime.datetime.now())
                # technically, the actual contents did not have to be
                # loaded (except for the print). 'message.content' is 
                # just the id, and we can just set a tree node to point
                # to this object id
                my_greetings[greeting_key] = message.content
    # persist the new step
    new_inbox_id = await store.store(inbox)
    core["my_greetings"] = await store.store(my_greetings)
    new_core_id = await store.store(core)
    new_step = Step(
        last_step_id, 
        step.actor, 
        new_inbxo_id,
        step.outbox, # we did not change the outbox
        new_core_id,
        )
    new_step_id = await store.store(new_step)
    return new_step_id
```
You see! Lots of boilerplate. But at least there is no magic; all the code follows from the Grit object model definitions above. The function has some minor side effects like `print` and using `datetime`, but it mostly consists of manipulating the Grit object store, first loading all the relevant state, then modifying it, then persisting the state by creating a new step.

Although there are synchronous versions of the `load` and `store` functions (i.e., `load_sync` and `store_sync`), it is advised to use the async versions whenever possible to avoid blocking the async message loop of the runtime.

### High-Level API
The APIs that a developer will use should be more ergonomic. There are two kinds of utilities to make writing state transition functions easier:  function wrappers (or decorators) and object model helpers that make it easier to work with the inbox, outbox, and core of a step.

In the Python library, the [data model classes](/src/wit/data_model.py) are called `BlobObject`, `TreeObject`, `Core`, `Inbox`, and `Outbox`. The `Core` class is a special type of `TreeObject`. All these types allow the developer much simpler access to the object store, new input messages, and sending messages to other actors.

The [function decorators](/src/wit/wit_api.py) wrap the Wit functions by moving a lot of the load and persistence work out of the function implementation, thus reducing boilerplate.

For example, here is the how the above code changes if we utilize the high-level API:
```Python
app = Wit()
@app.messge("greeting")
async def handle_greeting(greeting:str, core:Core):
    print("new message contents", greeting)
    # use the current datetime for the node key
    greeting_key = "{date:%Y-%m-%d_%H:%M:%S}.txt".format(date=datetime.datetime.now())
    # make a path to a blob and set the contents
    core.makeb_path(f"my_greetings/{greeting_key}").set_as_str(greeting)
```
The `core` object makes Grit modifications feel much more like standard filesystem operations: e.g., creating a file on a certain directory path, then saving contents to that file. Moreover, there is a lot going on under the hood [to route the message](/src/wit/wit_routers.py) to that function and make the message contents available in the variable `greeting:str`, and then to persists the modified core and produce the new step id. But what is hidden is also not much different from what we did above in the drawn-out example, and so the developer should be able to understand what happens between this level of abstraction and the low-level API.

### Wit Entry Point
A core can have whatever data in it, but the runtime expects the core to also define where to find the Wit code that gets executed.

In the root of the core, there must be a node called `wit` which defines the entry point for the Wit function. The contents of the `wit` node are largely programming language specific. In the future, there will be other nodes that define the runtime and package versions, but not yet.

Whenever the runtime wants to execute an actor, because there are new messages, it looks up the last step id from the references (`refs/heads/<actor id>` ), then loads the last step, finds the `wit` node in the step’s core, resolves the associated state transition function either by loading it from the core itself or externally, then executes the function passing it the last step id and the new messages. If the Wit function succeeds, the runtime updates the references so that `refs/heads/<actor id>` points to the last step id.

In Python, the `wit` node must be in the root of the core—i.e., the path must be `/wit`—and it can point either to external code or to internal code. External code just means the code gets executed from the filesystem and not from the contents of the core itself. This is only used for built-in Wit functions or during development. Internal means the Wit code is actually loaded from Grit and executed from there. The runtime has a custom Python module loader for this, and the runtime uses a custom function "resolver" to load the function depending on the Wit reference type. 

As for how code get's into Grit, we will discuss this later in the "Actor Lifecycle" section.


### Side Effects & Determinism
A Wit *can* have side effects. For some this might be anathema in an event-driven system.

First, we are decidedly not aiming to build an [event-sourced system](https://martinfowler.com/eaaDev/EventSourcing.html). Event sourcing means that the current state can always be recomputed from the events that went into the system. And for that to work deterministically, the functions that compute the state transitions cannot have any side effects, otherwise the final state will always be different. We do not believe recomputability is a desirable property, mostly because it is not worth the effort. Agents will live in the real-time digital world of their users. Events come and go, and we are only interested in maintaining the latest state.

Now, you can go back and recompute a step, but this is not something that we want to do with the entire history, only if the last computation crashed or the final step id was not persisted for some reason. In other words, we only use recomputability to retry Wit executions and to give more flexibility to the scheduler of the runtime. So, it is still advisable that a Wit function, insofar it modifies external resources, is implemented with safeguards in case the Wit is executed multiple times with the same messages.

As for modifications of the internal state of an actor when it creates a new step, this can certainly be implemented quite side effect free. But not fully, because we support programming languages that are neigh impossible to put into a deterministic straitjacket. 

We are trying to offer the best of both worlds in terms of determinism. Wit functions in their raw form are very easy to reason about (step in, step out), but they can have side effects and are expected to do so. For example, a Wit that calls out to an LLM will just import the relevant libraries and make the call itself, usually asynchronously. The problem with trying to make functions pure is that the complications of the world must be dealt with somewhere nonetheless, which usually means building complex support structures that color the entire stack. In the Agent OS, a Wit developer can choose to make certain functions pure and reason about them in a certain way and make others messy and involved and figure out how to deal with them separately.

Finally, it must also be mentioned that working with Grit produces side effects too: each time an object is stored, all kinds of I/O occurs. However, Grit is thread and multi-process safe, and since it is append-only, it cannot be corrupted. So, it is benign if a step runs multiple times, producing the same objects over-and-over. What matters in the end is the adoption of a new step id by the runtime, which is well-structured logic and easy to reason about.

### Wit Queries
Not all code that runs against a step must necessarily make changes to it. So, the Agent OS also supports read-only operations that do not run as a state transition function. We call these "queries."

Queries are possible because the data in each step is static because the object store is append-only. Consequently, if step N contains code to run queries against its own state, we can do that safely without worrying that a separate Wit execution will conflict with the query.

Now, a query is generally addressed to an actor and not a specific step. For a query, the runtime tries to use the very last step that has been generated by a Wit, but this is not guaranteed. If this is a dealbreaker, then one can always communicate via normal messages that are executed as part of a state transition.

Since a query is not a Wit, it has a different definition or protocol:
```
step + query name + query args -> blob or tree
```
Or in Python:
```Python
def wit_query(step_id:StepId, query_name:str, query_args:Blob) -> Blob | Tree | None:
```
Notice that a query returns data directly and not an object id.

A core can only contain a single query entry point via a `/wit_query` node, which is defined in the same way as a the `/wit` node. Consequently, it needs to differentiate the query through the `query_name` parameter. Also, specific queries need to be able to take arguments, which is the purpose of the `query_args`. The arguments are a blob because they can take any shape, but most of the time the blob contains a JSON dictionary that is structured like a normal HTTP request query string.

#### High-Level Query APIs
Just like with wits, there are also more ergonomic APIs for queries. Here is an example:
```Python
app = Wit()
@app.query("messages")
async def on_query_messages(core:Core, messagekey:str=None):
    message_filter = messagekey
    messages = await ChatMessage.load_from_tree(await core.gett("messages"), message_filter)
    return await render_template(core, "chat_messages.html", messages=messages)
```
One thing we haven't talked about is that there is also a web server. This particular query reads messages that are stored in the core and renders them using an HTML template that is also stored in the same core. Queries can be accessed through the web server, and if the resulting blob contains HTML, it is rendered accordingly. So, here the query serves an external endpoint for the user, but actors can also query other actors.

#### "Wut" Queries
As a convention, Wit queries should implement a special query name called "wut." This particular query functions as an actor API descriptor. It returns information about the messages and queries an actor supports.

This has two purposes. First, to generate automatic API specs for external clients. For example, it supports the generation of [OpenAPI](https://www.openapis.org/) specs.

Second, a different actor can use an LLM to make sense of the capabilities of an actor through its "wut" spec and so interact with it without having to have hard coded dependencies between them, such as shared message types and other code. We believe this is a forthcoming novel way of composing software components: a new actor can just be dropped into the system and other actors can learn to make sense of it and use it without manual coding. ChatGPT plugins work like that. The Actor OS aims to make extensive use of this pattern.

### Wit Summary
At the heart of the actor model is the Wit function, which is the name of a state transition function of the form: `step + new messages -> new step`. And since such a function produces Grit objects, the actor model is deeply tied into the object model of our persistence layer.

The actual code of a Wit lives inside the core of the step that is part of the input to the Wit itself. This kind of recursive setup makes it trivial for wits to update their own code, or other actors to update it through a message. In other words, the design is quite suitable for hot code reloading, which is a desirable feature for a system that will write its own code through LLMs.

However, it will not just be machines that write Wit code. Much of the API surface design work goes into providing a good development experience for people who will spend substantial time writing Wit functions and queries. Therefore, it is paramount that the high-level abstractions are ergonomic and powerful.

Before we look at how the runtime works, let's now consider the entire lifecycle of an actor.


## Actor Lifecycle
We now understand all the parts to fully consider the lifecycle of an actor.

Most of the time, actors are designed by developers. But how does the code get into the OS and start executing? Bootstrapping actors begins outside the Agent OS.

#### Development and Push
A developer writes a Wit function in an IDE and pushes the Wit and associated data regularly into Grit and executes it via the runtime. 

On the programmer's filesystem, at the root of the project folder, there must be a `sync.toml` file that describes which actors should be instantiated with what Wit function and initial data. The sync file basically describes your agent as a dev workspace. All relevant data and code that is mentioned in the sync file gets pushed to Grit.

Often there is a one-to-one correspondence between Wit and actor, but just like how in OOP a class can be instantiated many times, so too a Wit can be instantiated as different actors. All this is defined in the sync file.

There is a CLI that that pushes the code (and other contents such as HTML files or images) to Grit. For example, `aos -d my_agent/ push` would push the directory  `./my_agent` with the assumption that it contains a `sync.toml` file which points to other contents in sub-directories. (It's also possible to sync the other way, from Grit to the filesystem, for debugging and other reasons.)

Once Wit code has been pushed to Grit, and the runtime is started, the proper lifecycle of an actor begins.

#### 1) Genesis Message
It's not possible to just create an actor, one has to send a "genesis message" to a not-yet-existing actor, which then brings it into existence by executing the first step transition function. How does this work?

A genesis message is just a normal message as defined in the Grit object model. But the message contains the entire initial core of an actor, including the Wit code and any other initializing data. Concretely, this means that the content id of the genesis message points to a tree id which is structured like a core. Now, if you remember, the actor id *is* the object id of the *initial* core of an actor. So, we know who the recipient of the genesis message needs to be: the recipient id is same as the object id of the core. 

This is something that the runtime enforces. And when the runtime routes a message and realizes an actor doesn't exist yet, it creates it. In the case of a genesis message, the runtime locates the Wit to execute not in the last step, but in the core of the genesis message itself.
 
Consequently, a code push to Grit doesn't directly instantiate the actor, it just creates a genesis message. (Or, if the actor already exists, a push creates an update message.)

But it's not just external pushes that create genesis messages. Any actor can send a genesis message to any other actor, and it is harmless to send a genesis message more than once. It is fine if different actors, people, or other systems try to create the same actor. This is why we call it a "virtual" actor model, because if you know the actor id, you either learned it via message from somewhere else, or you know the exact contents of the core that creates the actor which is equivalent to knowing the actor id, and as consequence you can just try creating the actor before sending it other messages. 

#### 2) Subsequent Messages & Wit Execution
Once one actor knows the actor id of a different actor, it can start messaging it. It can send anything to the actor it wants, but the receiving actor does not have to accept the messages. Usually, actors define certain message types that they respond or react to, but that's up to the developer to define.

Anything that changes an actor—that is, creates a new step id—must be initiated via a message to that actor.

Actors are usually not long running and never run arbitrarily. They are only executed when they receive a message. Technically, an actor never just "runs;" the runtime just executes the Wit state transition function whenever there is one or more new messages. And that's all there is to an actor.

Whenever the runtime routes a message to an existing actor, it locates the previous step (using the references store), resolves the Wit function from the core of that step, and then runs the function with the new messages to create a new step (and then saves the new step id in the reference store). The only times where the runtime executes code from inside the message itself is when handling a genesis message or an update message, otherwise the code is from the previous step. 

#### 3) Updates
Since only the Wit function can change itself, we have a problem if we want to update the Wit function itself; especially if the Wit function is faulty. Hence, there is a special message type, aptly called "update," which is treated a bit differently than other messages.

Just like the genesis message, the runtime looks for a Wit in the message itself (not the previous step). Here, the optional core node is called `/wit_update` which should point to a special Wit transition function that does just updates but has the same signature as a normal Wit. It is optional because if the update message does not contain such update code, a default procedure is applied. The default procedure simply merges the tree in the message into the target code. Most of the time this is exactly what is desired. A custom update Wit is only needed if more complicated state upgrades are desired.

So, most of the time, the expectation is that an update message contains a partial core, or tree, with updates to the target core. From the example above, if we want to update `/code/greetings_wit.py`, we just send a new tree that contains just this file: `/code/greetings_wit.py`. (If these updates are initiated by the developer, the CLI `push` command will do the right thing.)

#### 4) Cleanup
Something that is not implemented right now, but is planned, is another special message type that prompts a Wit to do internal cleanup.

Most significantly, such a message should instruct an agent to create a new step that does not reference a previous step. Which will then allow the underlying Grit garbage collector, or pruner, to delete a lot of unused and unreferenced data that was generated in previous steps. Since the event log is append only, this is necessary. (There is a similar problem with messages that reference previous messages, i.e., are linked lists, but we'll not get into this right now, although we do have an idea how to solve it.)

The runtime will give special guarantees when it executes such a message, such as only running it when all other messages have been processed, and so on.

#### 5) End of Life
An actor is considered to be "end of life" if none of the outboxes of all the other actors point to it. The runtime then deletes the head step reference, which then allows the pruner to clean it up.

### Actor Lifetime
A quick note on the expected lifetime of an actor. Because we want our agent to be consistent and usable over a long time, the whole system is designed to make data persistence and Wit executions explicit in the step object. Actors are designed to be long-lived once they have processed their genesis step. Possibly actors will endure for many years.

If you shut down the runtime and start it again, the actors persist and will just continue operating, because their state is stored in Grit. Therefore, developers do not need to re-instantiate any actors on startup like they have to in traditional programs with objects or other data structures when such a program runs.

As an aside, you might wonder if we are not introducing reference counting or a manual ownership model to account for the actor lifetime? That is, the outboxes that have an actor as a recipient are the "references" to an actor and until those are set to null, the target actor cannot be reclaimed. This is indeed the case, and in that way, actors function a bit like objects in OOP: as long as one object references another, the referent cannot be garbage collected. It might seem like a serious complication, but it is tractable. Most of the time actors are very long-lived and so they don't need to be reclaimed ever. If an actor spawns other actors in, say, a fanout pattern, the Agent OS can provide libraries that ensure the pattern is implemented in such a way that the ephemeral actors get reclaimed properly.


## Runtime
The runtime is what ties Grit and Wit functions together. Its fundamental responsibility is to route messages between actors and execute the Wit transition function for individual actors once they have new messages.

### Current State
The runtime, as it currently stands, is very simple. It's about a 1000 lines of code and it runs itself and all actors in a single process. It is designed to run on a person's private machine.

The Python runtime makes extensive use of asynchronous programming. The reason for this is that many tasks will call out to model APIs (and other APIs), which all can happen asynchronously. There is no need to block a thread in long running I/O operations. So, the runtime focuses less on CPU-heavy workloads (although these are possible too), and more on I/O workloads. The assumption is that CPU or GPU heavy workloads, such as model inference, are hosted in stateless services somewhere else and not in the Agent OS itself.

For Grit persistence, the data store currently uses [LMDB](https://en.wikipedia.org/wiki/Lightning_Memory-Mapped_Database).

### Runtime Loop
The runtime consists of a simple algorithm.

  1) Before the runtime starts, it gathers any pending messages from actor outboxes that have not yet been applied to the recipient's inboxes. It does this by comparing all the outboxes and inboxes of all actors. (Remember, the inbox is really the "read inbox" of an actor.) The runtime then primes the internal message queue with those pending messages.
  2) It then looks for any persisted actors and creates a an "actor executor" for those. Each actor has its own executor. The runtime uses the reference store to figure out which actors should exist by reading all  `refs/heads/<actor id>` entries.
  3) The runtime then starts processing the message queue and waits if there are no new messages.
  4) Whenever there is a new message, it routes it to the executor of the appropriate actor. And if the actor does not exist, which is the case if the message is a genesis message, it creates a new executor for that actor.

The executor runs whenever there is a new message for its actor. Here is how it works:

  1) The executor knows the current "read inbox" of an actor, which is the inbox of the last step. It knows this because it can retrieve the step head from the reference store. And the step contains the last known inbox.
  2) It also maintains a "current inbox" for the actor, which contains the messages that the runtime has routed to it, but the actor has not processed yet.
  3) If the "read inbox" and "current inbox" do not match, the executor runs the Wit state transition function with the last step id and the new messages (see Wit definition above). If there is no previous step, because it is a genesis message, then the step id is empty or null.
  4) If the execution of a Wit function succeeds, there is now a new step id. The executor persists that step id in the reference store. With that, the actor has successfully progressed one step forward. And if the runtime would die after that moment, the actor would continue from that step next time. 
  5) However, there is one more thing to do, to keep the runtime spinning: the executor compares the outbox of the previous and new step to see if the actor sent a new message. If so, the executor calls back to the runtime with the new message(s), which then puts the new message into the runtime’s message queue. And we continue from step 4) in the runtime above.
  6) The executor then waits until it is signaled by the runtime that there are new messages for its actor.

### Error Handling
So far, we have touched very little on error handling. This is primarily because error handling is not fully thought through yet.

However, for queries this is easy, errors just bubble up to the caller, be it a different actor or an external system.

For Wit functions, errors are trickier, because everything happens through asynchronous messaging. Here we enter the treacherous territory of dead letter queues, poison messages, delivery of error receipts, and so on. But the current approach is that an actor should try to handle all messages, if possible, by catching errors, and marking even faulty messages as read. This is also known as the “dumb pipes, smart endpoints” principle. However, if an actor repeatedly fails on message delivery, the runtime will exponentially back off, until it finally marks an actor as irrecoverable.

We will still have to decide if there should be error receipts that go to the message sender, indicating that the recipient is not available (basically bubbling exceptions). 

If an actor becomes irrecoverable, the only option is to send it an update that fixes the error. The nice thing is that we could have "healing" actors that read error messages originating from a different actor and use LLMs to generate code fixes.

### Performance
The current performance is acceptable; the runtime can process about 5 Wit transitions per millisecond, or about 300,000 executions per minute. Since actor executions are expected to be quite granular, i.e., doing quite a bit of work per execution, this is sufficient for now to build highly concurrent and versatile agents that consist of possibly thousands of actors.

Still, the tidiness of the architecture comes at the expense of performance and efficiency, but it’s a tradeoff we are willing to make *at the level of implementing actors*. On the other hand, at the level of the Wit functions executions, e.g., when running Python code, this is not so much a problem because the penalty is only paid for those things that are persisted to the object store and which are split up between actors and thus need messaging. The rest of the code can make full use of the speed and optimizations of Python, or any other programming language that implements the Grit and Wit protocol. In other words, actors are at the right level of abstraction where to pay the toll of Grit persistence.

### Future Runtime
What is the future trajectory of the runtime? As stated previously, agents will have different parts running in different places. Some actors will have high compute needs, others should run on the cloud edge for fast response times, others will be chugging away on the user's devices.

The runtime, then, needs to be a system that guarantees the integrity of the agent as a whole by ensuring that the Grit namespace is available to all actors, no matter where they run. It also needs to coordinate the resource and sandbox requirements for individual Wit executions. For that, cores will likely contain more metadata about security and resource requirements in the future, and the runtime will need to ship the function to the right place to be executed, considering the constraints of those requirements.

So, conceivably, the runtime, in future iterations of the Agent OS, will function more like a distributed orchestration layer that lives somewhere in the cloud, providing the functionalities we just outlined. Much of the Agent OS is designed with this future purpose in mind. Most significantly, Wit functions are designed to be executed in a cloud function environment. Moreover, the object store is a just a very simple key-value database and can plug into distributed KV stores such as Foundation DB or other managed offerings. Larger objects will likely be stored in blob storage systems like S3.

You might also wonder about sandboxing specifically, especially when we talk about generating code and executing it. The Agent OS is designed to work in conjunction with current sandboxing systems, such as containerization and other Linux namespacing techniques. For example, a core could be required to carry a manifest of the type of I/O and external resources it wants to access, and the runtime will then make sure that the Wit is executed in a suitable environment. Further, the system is designed with WebAssembly in mind. It is very much conceivable that most Wit functions will be written in a WASM compatible language and that the runtime will utilize WASM for sandboxing.

### Security and Privacy
For data security and privacy, the runtime will provide all kinds of low-level primitives that make sure the data is secure from prying eyes. 

On the other hand, the idea that actors will run in different places is a key aspect to building agents that you can trust with your most private data. Your personal agent might be acting on all kinds of data and events in the cloud, but when it comes to, say, personal medical records you should have the option that such data can only be read and operated on by an actor that runs on a trusted device, such as your personal computer.

So, for true privacy, the [end-to-end principle](https://en.wikipedia.org/wiki/End-to-end_principle) applies. That is, the actor implementor will have to ensure that certain data is encrypted as blobs inside the object store, that such data is only sent to trusted language models, and so on. And the runtime can provide facilities that encryption keys are only available to permissioned actors.

As for code security, the runtime will largely be tasked with the responsibility to make sure that only permissioned changes are made to the cores of an actor. For example, there might be actors that have a "frozen" codebase that cannot update itself. When the runtime detects a step in such an actor that modifies its own code, it can just reject the step and not make it part of the head reference, thus avoiding ever executing the modified code. The orderly step execution regime is suitable to add all kinds of security and privacy enforcement subsystems. 

## Runtime Services
The runtime will also provide a few services that are useful for actors. For example, the runtime already offers a web server that can serve the contents of a core as JSON or HTML.

Some of those services will be implemented as special actors, others will live outside the actor model. A lot of these services are still tentative. Most of the services listed here are not strictly required because actors can roll their own because they can have side effects, but it might be helpful to offer these abstractions to avoid duplicating efforts.

### Runtime Actor
There is a special actor that represents the runtime. It's primary function, right now, is to be the entry point for external messages that are injected into the agent by a developer or by the webserver.

### Web Server
The Agent OS comes with a [built-in web server](/src/web/web_server.py). The basic intuition is that it is trivial to render any blob or tree in Grit as either JSON or a full-blown website. Combined with Wit queries that can transform core data on demand, the web server can supply most frontend affordances.

The Agent OS does not come with any UI (yet). The idea is that the UI is shipped as part of an individual agent implementation. For example, if you look at the [code of the "first" agent](/examples/first/frontend/), you see that a simple chat interface, implemented using HTML, some JS, and HTMX is quite straightforward.

The goal is also that Grit has a HTTP protocol just like Git does, but this is not fully implemented yet.

### Secrets
There will be a service that manages access to secrets, especially encryption keys.

### Timers
There will also be a service that handles timers for other actors. This will likely be implemented as a special actor similar to the runtime actor.

### LLMs & Other Models
We might create special facilities for actors to call to externally and internally hosted models. This is still to be determined.

### Agent Network
It's very conceivable that agents want to form a communication network. If that's the case, there might be a sub-component that routes messages to other agents and allows actors to share data from actors of external agents. The content addressable storage system, especially if hosted in the cloud, could make this very simple.


## Conclusion
The Agent OS is a relatively simple system. It proposes two basic primitives: the Grit object model and the Wit state transition function. Based on these two things, we believe, it should be able to construct powerful autonomous agents.

In the end, data are just blobs of bytes. It used to be the case that it was necessary to build extremely intricate programs to transform data from one shape and purpose to another. But with the advent of LLMs, the semantic structure of data is much more readily accessible and can yield its own programs by being understood by language models. Actors will make extensive use of the flat data structure of Grit and pass much of the contents along to external models to make sense of what these blobs contain—be it text or images, or other formats. The models, in turn, will respond with code, not just explaining what the data means, but what to do with it. The actors can then reify that code as Wit functions and instantiate them as new actors that participate in the ecosystem of the agent.

Most of the goals put forth on the outset are directly solved by different aspects of the architecture:

  - Goal #7: It's a system built for reactive programming, meaning it can react to all kinds of events, even when the user is not around.
  - #6: Actors can run concurrently, and so many agent-internal processes and tasks can happen at the same time. 
  - #5: We covered extensively how code can live inside an actor's core and be created and modified, which covers the foundational problem of bootstrapping and instantiating code in real-time, while the system runs. 
  - #4: Grit, with its append-only storage where all data is clearly referenced in a DAG is suitable for long-term storage because we can reason clearly about what data is still in use and what can be reclaimed. 
  - #3: With the notion of running actors in different places and environments, there is a clear idea how we can both give an agent a lot of computing power in the cloud while keeping the most private data local or in other exclusively trusted environments. Combined with proper encryption schemes, a developer should be able to build an agent that has the best of both worlds.
  - #2: Grit is eminently scalable. It is designed for terabytes of data. You will be able to feed it anything that you ever see or come across.

Finally, goal #1: Will the agent truly be yours and work for you? In many ways that is up to the agent developer; the Agent OS cannot guarantee it. But it can provide the computing primitives that make it more likely to be true. The system is simple enough that even novice programmers can hack on it. It is also very auditable and inspectable since the data and code live side-by-side inside the cores of actors, which makes it at least possible for the user to crack open the hood and make sure things work how they expect.
