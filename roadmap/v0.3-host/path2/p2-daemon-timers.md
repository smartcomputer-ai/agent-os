# P2: Daemon Mode + Real Timers (path2)

**Goal:** Turn batch runtime into a long-lived host with working timers and graceful shutdown.

## Host Loop

```
WorldDaemon {
  runtime: WorldRuntime,
  timer_heap: TimerHeap,
  control_rx: mpsc::Receiver<ControlMsg>,
}

loop select {
  ctrl msg   => enqueue event/receipt/proposal, trigger drain
  timer due  => inject TimerFired, drain
  idle tick  => drain if work pending
  shutdown   => snapshot + exit
}
```

## Timer Adapter

- `TimerAdapter::execute(timer.set)` schedules in `TimerHeap` (BinaryHeap by deadline).
- Immediate OK receipt for the set; firing later becomes `sys/TimerFired@1` event injected by host.
- `TimerHeap::next_deadline()` used for `tokio::time::sleep_until`.

## CLI

- `aos world run <path>` → spawns daemon loop, registers adapters, listens on local socket/stdin control channel.
- Ctrl-C → broadcast shutdown → final snapshot.

## Tasks

1) Implement `TimerHeap` + `TimerAdapter` (real deadlines, not stub).
2) Add `WorldDaemon` with tokio select loop.
3) Control channel MVP: JSON over stdin/stdout or Unix socket (`send-event`, `inject-receipt`, `shadow/apply` later).
4) Hook graceful shutdown (Ctrl-C).
5) Logging via `tracing-subscriber` (structured, human-readable).

## Success Criteria

- `aos world run examples/01-hello-timer` fires timers at correct wall-clock times.
- Clean shutdown writes snapshot; restart and replay works.
