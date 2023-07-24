workers are like runtimes, but simpler. They recieve messages from the runtime of what actors to run. Usually, it's one worker per process.

Worker also use virtual steps to manage its state and are thus introspectable.