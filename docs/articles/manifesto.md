# Agent OS Manifesto
*August 2023, Lukas Buehler*

## Why an "Agent OS"

We are entering the age of agents. It is now possible to build a personal agent that can accomplish a vast array of tasks on your behalf. Such an assistant will be able to ingest and categorize most of the information you come across online, act on relevant pieces of data, use the history of this information as context when asked to do stuff, and be an entertaining and insightful conversational partner. In short, you'll have an assistant that manages your digital life for you.

The latest generation of large language models (LLMs) are immensely capable, but these capabilities are rarely bundled into tools that persists your intentions, configurations, modifications, and history in a single place. Each ChatGPT session starts with blank slate; you cannot configure it much, nor does it learn about you. And more importantly, your assistant should not be solely conversational, answering questions in a ping-pong fashion; instead, it should always be on, listening and watching your data streams, and acting on your behalf when necessary.

But to do this an agent needs to have access to a computer, or rather a “compute environment”. Most of the actions of your agent are going to be expressed in code. You'll ask it to do something, and it will generate the necessary code to do it. That code, then, needs to run somewhere and be orchestrated with other programs the agent has previously created. Over time, an agent will build a huge library of programs that are continuously running on your behalf, and it will also manage those programs for you.

Building such a compute environment is no easy feat. It must ensure that the agent does the right things at the right time with the right permissions. In fact, this computer, in conjunction with the models, *is* your agent, not the language model alone. But because there are so many things to orchestrate and organize, we cannot think of this agent as a simple program. We should think of it as a whole system that combines different components that make this system feel like an assistant with agency. In many ways this compute environment will work like a contemporary operating system; hence we can call it an "agent OS".

Of course, the realization that agents are now possible is not something new—many people are talking about it—but what are the concrete properties of this computing substrate that makes agents a reality?

## Desired Properties of an Agent OS

The goal is to build a personal agent that you can trust that it is working *for you* and not someone else. You need to be certain that an agent only acts on your goals and intentions and not those of some other party or company. If we design an OS for agents, we need to consider this goal as foundational, even if it is in tension with other desired properties.

### Orchestrate Models

I believe the models themselves will not be part of the OS itself. We need to think of them as "peripherals" or "infrastructure" in the same way an OS treats the CPU as an abstraction. An agent will call out to various models that are hosted either by a third party, like OpenAI, or hosted locally, such as various [Huggingface Transformers](https://huggingface.co/docs/transformers/index) models.

Consequently, the system needs to orchestrate the use of various machine learning models. For an agent to work well, it needs to know how and when to utilize different models. For example, one model will be best for planning, another for code generation, another for scientific research, and another to generate images or audio, and so on. The OS must have configurable logic that knows what model to use for which intent.

### Your Context

However, models are only as good as the data you feed it. LLMs need to be prompted properly, and language models are stateless. Each request to a model needs to include all the relevant context; otherwise, you don't get good answers. For example, when you use ChatGPT, each time you ask it something, all the messages up to this point get passed into the API call to the model. If you are using ChatGPT, have you noticed that conversations only get really interesting quite a few prompts in, when you have properly established the full context for the conversations, i.e., made all the qualifications and dug into all kinds of details?

So, a personal agent is also your context manager. It will be a key responsibility of the agent OS to collect and aggregate all the relevant data so it can fulfill the user’s intent without having to ask a myriad clarifying questions. The system primes the LLM pump for you instead of you having to supply all that information.

### Privacy and Data Ownership

For that to work well, the agent needs to know a lot about you. The more the better.

However, we have come to distrust any system that knows too much about us, except, perhaps, our personal computing devices, like our mobile phones. Consequently, in order to build personal agents, we need to find a way to make them work that you feel like you can trust them with your personal details.

The instinct of many people working in the personal AI space is to bring the agent and the models as close to home as possible. This is certainly the most straight-forward way to do it, but it also has problems. For once, it is eminently impractical but for the most technically adept. If you want to go all the way, you have to host several models on your own device and also host the agent code and all your data with it.

I believe the best solution is hybrid. Some of the data and prompts should live on trusted devices (or server) and be sent only to high-trust model providers. For example, if you want to use LLMs to examine and discuss your medical history, you might want to tag that conversation confidential, which should kick the agent into a special mode where it doesn't send data to OpenAI, but only to a locally hosted model.

On the other hand, if you want to discuss, say, the history of boat building with your agent, it might be fine to use relatively ubiquitous third-party models. And of course, there is everything between these two extremes. In all cases, the agent should follow your preferences. Now, it is my belief that most will tend towards convenience but will want to reserve the option to do high-trust, highly isolated personal AI.

It should also be mentioned that sending individual prompts to external models does not leak *all* your data, only some of it. The model provider could try to piece it all together, but they likely won’t, and some say so explicitly. In other words, the thing to worry about the most is not the model provider but the computing substrate of your agent, because it has all your data. So, the nature of the agent OS matters more in terms of privacy than the model providers.

### Always-On and Reactive

Current "agents" operate mostly in a ping-pong, prompt-response manner. Once you close your ChatGPT window, nothing more happens. But if these models are smart and can make plans and various consideration on actions to take, you want them to actually *do* them whenever a relevant event happens. Your assistant should be able to react to many more events than just your chat input. For this the Agent OS needs to be always on and always available.

I think the word "agent" is used misleadingly today. ChatGPT is not a true agent, it's a powerful model with a thin chat layer on top (admittedly this is changing with plugins and code interpreter). A real agent has agency. And we need to build systems that safely realize the agency inherent in large language models. In fact, the nexus of agency, is not the model itself—it's just a potential. The place where the agency is realized is the substrate where the agent runs its programs.

But what does agency mean here? I believe it consists primarily of two dimensions. One, the ability to react in real-time to various events, from emails, to calendar invites, to timers, to chat or voice input. And second, being able to take general action by writing code that solves a task or problem. Today there are many systems that can digest a lot of different inputs, so arguably the magic really comes from the second aspect, and we'll get into that in a moment, but I also believe the agent OS needs to take specific care to be designed for reactivity.

### Cloud vs. Local

A takeaway of this is that your agent will run mostly in the cloud. It's just way more convenient that way. The assistant can always be on, processing your data streams, even if you are sleeping. Plus, the agent needs to be able to handle a large amount of data for you and do it quickly. These are all things that are much easier if your agent runs in the cloud.

But how can we provide you with much stronger guarantees than current cloud software does, which often mines your data?

I think personal agents will be a key driving force in creating a new computing paradigm, called "personal cloud compute." Currently, large companies enjoy the privileged position where large cloud companies treat them truly like old-fashioned customers. The cloud providers don't spy on their corporate customers, and they don't resell their data. The providers just sell the companies compute, data storage, and some other services, and that's it. This is what we want for everyone too. If that kind of relationship were possible, running our agents in the cloud would be very much desirable.

A few things are needed for that to work. The runtime that executes your agent code and manages your data needs to be structured in such a way that you, the user, has more of a hosting relationship with the cloud provider. Simply put, there must be a sufficiently strong separation between the agent OS and the infrastructure of the cloud provider so that the provider operates more like a utility.

Second: not your keys, not your agent. All your personal data must be strongly encrypted where you are the custodian of the keys (at least optionally since not everyone might want this).

At the same time, cloud compute cannot really be made fully private—at least not at scale for retail users. So certain things better remain on your private machine. Consequently, the agent OS needs to be designed with the option to keep certain parts of an AI assistant local, or even the entire thing.

I do not think that all parts of all agents will be "local-first", because of the convenience of the cloud, but those parts that must run locally should be truly first-class citizens on your local device, meaning all the machinery runs locally and is persisted locally. As a nice bonus, most of the data can be synced back to the cloud if the data is encrypted and the keys do not leave the local environment. With local compute, end-to-end encryption of your most privileged data becomes trivial (because fully encrypting data that computes in the cloud is very difficult).

### Self-modifying Code

Agents will write, modify, and execute their own code. This is not how software works right now. Currently, developers write code on their machine and then this code gets pushed to production where it stays static until the next deployment. The data changes, but the code doesn't.

If an agent wants to do something, how will it do it? It will ask a model to generate code that expresses an intent or task and then it will execute that code. If the code doesn't work, it will modify that code until it accomplishes the intent. We see inklings of this already with things like [AutoGPT](https://github.com/Significant-Gravitas/Auto-GPT) or [BabyAGI](https://github.com/yoheinakajima/babyagi).

Now, it is possible that we give agents access to a Linux box and let them rip. But it is very likely that this won't work in the long term. It might work for a single program, but not for an agent that writes many programs and needs to compose them. The execution environment needs to be more constrained and designed for self-executing systems. (What we are really looking for is homoiconicity or at least hot code reloading, a single level store, and so on, but more on that at a different time). There is a venerable lineage of such systems, most notably various Lisp environments and more recently Urbit.

What is clear is that each agent's codebase will diverge from the code of other agents. This requires a different approach than traditional software development offers. And if we want to build personal agents that are accessible to everyone, we need to find a novel way to build and deploy software.

One idea is to combine the storage of code and data into one layer and ensure these two things evolve together in an orderly fashion. For example, with Git, we have learned to tame complexity with concepts such as repositories, commits, merges, and pull requests. I propose we build something similar that can host both code and data.

### Parallel Compute

Finally, an agent should be able to have many "though processes" in parallel. Perhaps, similar to humans, an agent will have parts that run concurrently and certain parts that are synched up in a "consciousness layer". That is, there might be a part that needs to ensure that everything fits together by making one decision at a time. But there are centrally many types of actions that an agent can take in parallel. For example, an agent should be able to deal with multiple emails at the same time.

In fact, if your AI assistant is doing a lot of work for you, it might have many more processes than fit on a single machine running in the cloud, which are all laboring on different aspects of your digital life that . The OS should make this possible, while also allowing consensus to be formed and things to converge again.

## Conclusion

If we realize the goals outlined here, it should be able to build agents that really work for you and belong to you.

I think these are important considerations to make because we are rapidly entering the "age of agents". Powerful AI agents will certainly exist, but who will they answer to? The agent OS should be a system that makes it possible to build powerful, long-lasting, autonomous agents that you can work with and evolve over time.

If you want to learn more, please check out the ["agent-os" project on GitHub](https://github.com/lukebuehler/agent-os).