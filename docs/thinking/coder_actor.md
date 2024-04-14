# Code Generation Using LLMs

For automatic code generation, I can think of three types of function genereation with increasing complexity.

First: Task description, plus all relevant inputs, plus specified output format -> code (which includes the inputs) -> execute code (with no additional inputs) -> result of execution -> print or display result

Second: Task description, input decription, output description -> code -> execute code with arbitrary input (but of described input shape) -> print/display results

Third: Task description, give some limitations on possible inputs and outputs (i.e. describe the framework) -> code, described inputs, described outputs -> execute code somehow matching the described inputs and outputs (probably needs another LLM) -> print/display results

## Types Function Generators

The function that utilizes an LLM to generate a function is a higher-order function that returns a function.

There are two types of generators: ones that also generate the input and output structure of the function, and ones that take an input and output structure and generate a function that takes those inputs and produces those outputs. Let's call the first ones "open structures"
and the second one "fixed structures." For the first, open type, we also need another higher-order function that that can take the generated function and figure out how to call it. For fixed functions, it is assumed that the user of the generator will just call it with the arguments it specified and will know how to process the outputs.

### 1) Generators where the inputs and outputs are externally specified

```
gen_func(task_description, inputs, output_spec) -> (function() -> output_struct) 
```

The desried output structure is exactly specified. The function can be executed with no additional inputs because they are part of the generated function. So, the function works a bit like a closure since the inputs are "capured" by the generated function.

The generated function does not strictly need to return a value because it could have side-effects. In the case of a function that is part of a Wit, it likely will not return a value, but rather produce a value in the core, as described, or send a message to a different actor, or both. But if it's part of an LLM flow, the outputs will likely need to feature in the history of a larger conversation that the overall progress can be tracked.

Nullary functions are likley the best way to do one-offs, because they are simple but not reusable (because the input cannot be changed). Although if the function can have side-effects, it could contain code that initially loads the current state. Such functions would likely also operate at the highest level to be utilized as a kind of "main" function.

```
gen_func(task_description, input_spec, output_spec) -> (function(input_struct) -> output_struct) 
```

The structure of the inputs and outputs are exactly specified and the arity is greater than 0. 

This type of generator can likely be conflated with the previous type simply by making the input spec optional, and moving the input decriptions to the task decription. And so if there is not input spec, the function will not take an input. Same for the output spec. If there is no spec, the assumption is that the function will produce its outputs as side-effects, which are described in the task description, and the function returns unit/void.

Also, if the generator actually writes an entire, say, Python module to create the single function, there could be other functions that the final function calls that take arbitrary inputs and generate outputs, but that is of no concern as long as the entry function is clearly of the open or closed type.

### 2) Generators that define the structure of the inputs and outputs

```
gen_func(task_and_env_description) -> (function(input_struct) -> output_struct) 
```
The generated input and output specification or schema and be derrived by inspecting the generated function signature. The task decription needs to include some description of the execution environment so that the LLM genereates usable code. That is, code that plays well with the larger system.

This type requires wrapping in a higher order function that generalizes to how to call a function where the inputs and outputs were derived by the LLM. One option is to limit the types of inputs and outputs sufficiently that a simple wrapper with some type and signature analysis can do the job. But truly open functions will need an LLM call to generate a function call completion.

## Libraries
Just like there are generators that take an external specifiction or create a specification, so too are there generators that take an exact description of the environment and available libraries and only use those. But there are also generators that return a list of libraries they want to use. In other words, they describe the environment they need to run in.

The self-describing functions also need a "wrapper" that sets up the envioronment that the genertor requested. This is, perhaps, a more broader problem than wrapping an open function to call it with the right arguments.

## Current Prototype
For now, we'll focus on generating closed functions and externally providing the list of avaible libraries.

An LLM will call the generator (as an chat completion function call) when the LLM believes it knows enough about the desired task. It will then wait for the function generator to complete and then call the function with the arguments it believes it derrived from the conversation. 

The generator is a separate actor that gets spun up for the task at hand and will communicate with the chat-loop via messages.

(the chat loop can keep track of a "session" variable which lets the LLM choose which function we are talking about if we are working on more than one generated function. This allows us to have an "instance id" for each generator actor and use it to create it.)

The generator takes in a task description, an input specification and output specification. The input and output are the messages the generated wit accepts and produces.

The generator works as part of the wit function, or rather, it wraps the wit function and if a spec message arrives it generates or updates the function.

### State Transitions
Code generation is best modeled as a state machine.

Spec/Planning Phase
-> see if we have enough information to accoplish the task
-> search environment for libraries that might be helpful to accomplish the task
-> decide if sub functions are needed, create those first, start nested code gen with smaller scope
Coding and Testing Phase
-> generate code using the plan
-> errors compiling: retry 3 times, otherwise go back to planning
-> succeed -> emmit success message and forward to deployment phase
Deployed Phase
-> accept messages and execute code, produce outputs
-> if errors, go back to coding phase (but only if there are certain types of errors)
-> update spec/plan arrives -> go back to planning

This should probably be implementd using nested state machines for each major phase.
However, in the first version, let's keep things simple and just have a single state machine.

Thefefore, the most simple state machine, I can think of, that still achives the goal is this:

```
new
-> new spec
specified
-> plan
planning
-> gen code
coding
-> 
testing (compile, run in sandbox)
-> on error or issue -> back to coding (at least 3 times or so, otherwise ask for new spec and fail) 
                     -> to failed 
deployed
-> on error or issue -> failed
-> on execute, -> stay on deployed 
               -> or, if "is_job": completed
completed
failed
-> on new spec -> back to specified
```

In the first, version leave the "job" variant out, because a job is just a special case of the more general case which is to write an open and reusable function.
Also, a nice variant would be if it would take feedback on the plan by the user and only then start coding, giving the actor a bit of a command and control feel.


## Refs
- https://github.com/paul-gauthier/aider
- https://docs.sweep.dev/blogs/sweeps-core-algo 
