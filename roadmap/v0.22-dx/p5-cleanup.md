
## workflow abstraction
we need some sort of abstraction to run workflows that are implemented differently: builtin, wasm, python (future)

We do not have any builtin workflows right now, but we want to move aos-sys to building workflows.

## aos-sys
We should simply move the workflows defined in aos-sys to builtin workflwos (which are not supported currently, but should be).

These can likely be directly implemented in the kernel for now.

Then remove aos-sys crate.

