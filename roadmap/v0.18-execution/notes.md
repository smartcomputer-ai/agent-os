These are mine (the user's) notes, do not delete.

```
pub type OpenEffectSnapshot = WorkflowReceiptSnapshot;
```
why?

---

```
//todo: plan an workflow types in effect sub params
```
plan is not needed anymore


---

remove `WorldHost::run_cycle()`
and remove WorldHost?

---

- shared resources? effect runtime, ingress, etc? we might have dozens to hundreds of worlds per worker (in aos-node-hosted)


---
Goal:
- consume kafka contiusly
- full async handling of effects, including restore, end to end.

---

try removing aos-runtime

--- 

search for mentioning of "plan"