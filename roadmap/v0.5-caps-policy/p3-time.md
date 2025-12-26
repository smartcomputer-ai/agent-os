# p3-time: Deterministic Time (Caps + Policy)

## TL;DR
Caps, policy counters, and approvals all need a deterministic clock. The kernel must never read host wallclock; time only advances through **journaled receipts**. Use `journal_height` for ordering and a monotonic `logical_now_ns` derived from trusted receipts (timer receipts are the first source).

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
   - Monotonic time maintained in world state.
   - Updated only from trusted receipt timestamps (e.g., `sys/TimerSetReceipt@1.delivered_at_ns`).
   - Replayable because receipts are journaled.

---

## Update Rule (logical_now_ns)

On each receipt:
- If the receipt includes a trusted timestamp `t_ns`, set:
  - `logical_now_ns = max(logical_now_ns, t_ns)`.
- If no timestamp is present, leave `logical_now_ns` unchanged.

This keeps time deterministic and monotonic without host wallclock access.

---

## How It Is Used

### Caps (expiry)
- Evaluate `expiry_ns` against `logical_now_ns`.
- If `logical_now_ns` never advances, expiries will not trigger. This is deterministic; production setups should ensure timer receipts (or other trusted time receipts) flow through the journal.

### Policy (rate limits, approvals)
- Prefer `logical_now_ns` for real windows and approval timeouts.
- `journal_height` remains available for strict deterministic windows that do not depend on time.

---

## v0.5 Target

- Add `logical_now_ns` to world state.
- Include `journal_height` and `logical_now_ns` in the policy/cap authorizer context.
- Advance `logical_now_ns` from timer receipts (and later any other trusted receipts that carry timestamps).

---

## Notes
- Do **not** reinterpret `expiry_ns` as journal height. If height-based expiry is required, it should be explicit (e.g., a separate field or policy-level rule), to avoid semantic drift.
