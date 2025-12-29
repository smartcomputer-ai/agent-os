# p3-time: Deterministic Time (Caps + Policy)

## TL;DR
Caps, policy counters, and approvals need a deterministic clock. The kernel is the timekeeper:
it stamps each ingress with both **wall-clock `now_ns`** and **monotonic `logical_now_ns`** and
journals them. `logical_now_ns` is the authoritative time for timers/expiry/policy; `now_ns`
is informational for context only. Timer `deliver_at_ns` is in **logical time**.

---

## Goals
- Deterministic and replayable.
- Monotonic (never moves backward).
- Suitable for expiry checks, rate-limit windows, and approval timeouts.

## Non-Goals
- Real-time accuracy guarantees.
- Timezones or calendar math.

---

## Time Signals (Deterministic Inputs)

1) **journal_height**
   - Always available, strictly monotonic.
   - Useful for simple “N per K steps” rate limits.

2) **logical_now_ns**
   - Monotonic kernel time recorded at ingress.
   - Replayable because it is journaled alongside each entry.
   - Authoritative time for timers, expiry, and policy windows.

3) **now_ns**
   - Wall-clock time recorded at ingress.
   - Exposed to reducers/pure modules via context, **not** used for policy/cap enforcement.

---

## Update Rule (logical_now_ns)

On each ingress (event submission or receipt injection), the kernel samples its
monotonic clock `t_ns` and sets:

```
logical_now_ns = max(logical_now_ns, t_ns)
```

The journal entry records both `logical_now_ns` and `now_ns`.
Receipt timestamps are advisory and do **not** drive the clock.

---

## How It Is Used

### Caps (expiry)
- Evaluate `expiry_ns` against `logical_now_ns`.
- If `logical_now_ns` never advances, expiries will not trigger. This is deterministic; production setups should ensure the kernel clock advances on ingress.

### Policy (rate limits, approvals)
- Prefer `logical_now_ns` for real windows and approval timeouts.
- `journal_height` remains available for strict deterministic windows that do not depend on time.

---

## v0.5 Target

- Define `timer.set.deliver_at_ns` as a **logical time** deadline (monotonic).
- Add `logical_now_ns` to world state (monotonic kernel time).
- Journal `now_ns` (wall clock), `logical_now_ns` (monotonic), and entropy at ingress
  by extending existing journal record structs (no new envelope).
- Include `journal_height` and `logical_now_ns` in the policy/cap authorizer context.
- Expose `now_ns` and `logical_now_ns` in reducer/pure call context (`p3-context`).
- Align the timer system to kernel time (privileged adapter or kernel-assisted timers).

---

## Notes
- Do **not** reinterpret `expiry_ns` as journal height. If height-based expiry is required, it should be explicit (e.g., a separate field or policy-level rule), to avoid semantic drift.
- `deliver_at_ns` is logical time. If a reducer wants wall-clock alignment, it should
  translate using its context: `deliver_at_ns = logical_now_ns + max(0, target_wall_ns - now_ns)`.

---

## Required Code Changes (Prep for p3-context)

1) **Journal records**
   - Extend `DomainEventRecord` and `EffectReceiptRecord` with:
     - `now_ns` (wall clock, u64)
     - `logical_now_ns` (monotonic, u64)
     - `entropy` (fixed-length bytes, e.g., 64)
   - Fill these fields at ingress (event submit / receipt injection).
   - Replay uses recorded values; no host clock reads during replay.

2) **Kernel clock**
   - Provide a kernel time source (wall + monotonic) used only at ingress.
   - `logical_now_ns = max(logical_now_ns, sampled_monotonic_ns)` on each ingress.

3) **Timers**
   - Schedule timers against `logical_now_ns`.
   - Timer receipt `delivered_at_ns` should use kernel `logical_now_ns`.
   - Daemon/test timer paths stop using wall clock for scheduling decisions.

4) **Context**
   - `p3-context` will read the journaled `now_ns`, `logical_now_ns`, and `entropy`
     and pass them into reducer/pure invocation context.
