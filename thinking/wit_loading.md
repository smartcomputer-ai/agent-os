
# How Wits are Loaded

## Selecting the Runtime
The core may have a `runtime` node in the root of the core that gives information about which runtime to use to execute the wit.
If there is no runtime, then assumption is the current runtime is desired (basically local execution mode).

For Python, a resolver then kicks in that tries to load the wit function and queries.

A wit can either be defined in the core itself or just be a reference to a wit function that is available in one of the modules in the global namespace.

## `/wit` node

The resolver requires a node called `wit` at the root of the core. If it doesn't exist, the core is not valid. This wit node is the entry point for executing the next step of an actor.

The `wit` node must be a blob contianing text. It can be either a pointer to a wit function, or a wit function itself. If it's a pointer, it can point to an external wit in the runtime environment, or to code in the core itself.

An external pointer to a wit has the following contents in `/wit`:
```
external:wit_ref
or
external:module_name[.sub_module_name]:wit_function_name
```

If the wit is loaded from the core itself (which is the default), it must be formatted like this:
```
/code:module_name[.sub_module_name]:wit_function_name
```

And if the `wit` node contains a function directly, it can look like the following, which will be exec'ed:
```
def wit_code():
    ...
```
A code wit needs to define a valid wit transition function, see more below. When exec is called on the wit code, the `global()` context is used.

## Wit Resolution by External Reference

To make life easier during development, a core does not have to contain the actual wit code itself. It can just point to it. So, if it's an `external:` pointer, it will first look in the local resolver table if there is a manually registered wit that matches `wit_ref`. 

If it points to a `module:function`, it will look in the currently loaded python modules. It is the developer's responsibility to make sure that the module path is in `sys.path`. These paths can also be added to the resolver, with the `add_wit_path` function.

## Wit Resolution from Core

If the wit reference points to it's own core, the resolver will look in the core for a wit function. 

Core references must start with a slash, `/`, and the shortest core path is just a single slash, indicating the whole core can be searched for modules. Although it is best practice to put all code into a `/code` subnode.

If the wit point to it's own core--this is, it does not have an `external` reference--then it must contain at least one python module that contains a wit transition function.

All the python modules, which are nodes ending in `.py`, are loaded by a custom loader. If no code can be found, the resolver throws an error.

For core pointers, only the `module:function` format is supported.

## Step Execution

Before a step can be executed, the resolver needs to inspec the core and follow the rules oulined above. The goal is to find a wit transition function in the core that can be executed. If no wit transition function can be found, the resolver throws an error. It uses the core of the last step (or the core of the genesis message) as a target for inspection.

## Loading from a Core

See: https://docs.python.org/3/library/importlib.html#module-importlib.abc

Howto: https://stackoverflow.com/questions/43571737/how-to-implement-an-import-hook-that-can-modify-the-source-code-on-the-fly-using


Probbaly the easiest is to load all modules in the core wit path (and keep track of these modules), this then makes it easier to reload the modules when the core changes.

Use a method like this:
```python
import sys
import importlib

def reload_module(module_name):
    if module_name in sys.modules:
        importlib.reload(sys.modules[module_name])

#or
import sys
import importlib

def force_reload_module(module_name):
    sys.modules.pop(module_name, None)
    return importlib.import_module(module_name)

```

## Hot reload
Is doable because of the strict state transition function style.


