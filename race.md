# Scheduler Race/Liveness Roadmap

## Scope
This document tracks scheduler concurrency work in two phases:
- Phase A is the current hotfix release gate.
- Phase B is architecture debt and performance follow-up.

No public API changes are planned in either phase.

## Phase A (Now): Shutdown/Lifecycle Correctness + Observability

### A1. Deterministic shutdown ordering
- Priority: P0 correctness, P1 liveness
- Symptom class: hang during shutdown, first-spawn stall
- Status: hotfix target
- Work:
  - Set `sched_shutdown_flag=1`.
  - Immediately enter worker0 drain loop.
  - Drain runnable work until `active_coros==0`.
  - Join workers `1..N-1` after drain.
  - Stop preemption last.

### A2. Worker exit contract hardening
- Priority: P0 correctness, P1 liveness
- Symptom class: worker exit races, stranded runnable coroutines
- Status: hotfix target
- Work:
  - Worker loop exits only after observing shutdown and finding no immediate local runnable work.
  - On loop exit, always swap back to host context exactly once, then return thread function.
  - Remove trap/falloff-style shutdown exits from normal path.

### A3. Shutdown routing invariants
- Priority: P0 correctness
- Symptom class: missed wakeups during shutdown
- Status: hotfix target
- Work:
  - Keep spawn enqueue routed to worker0 when `sched_shutdown_flag==1`.
  - Keep wake enqueue routed to worker0.
  - Keep shutdown drain loop stealing from every worker each idle pass.

### A4. Debug instrumentation and counters
- Priority: P1 liveness
- Symptom class: opaque stalls and leak diagnosis difficulty
- Status: hotfix target
- Work:
  - Debug phase logs:
    - `shutdown:start`, `shutdown:enter_drain`, `shutdown:join_workers`, `shutdown:done`
    - `worker:start`, `worker:seen_shutdown`, `worker:exit_loop`, `worker:return_host`
  - Per-worker debug atomics:
    - `last_phase`, `in_coro`, `local_deque_size_snapshot`
  - Global debug counters:
    - `spawned`, `freed`, `blocked`, `woken`

### A5. Coroutine terminal-path guardrail
- Priority: P0 correctness
- Symptom class: active coroutine accounting drift, never-freed terminal coroutine
- Status: hotfix target
- Work:
  - Keep `active_coros` decrement in `coro_free` only.
  - Add debug assert/log when a coroutine reaches `CORO_DONE` without `co->fn == NULL` sentinel transition.

## Phase B (Later): Architecture Work

### B1. Safe deque memory reclamation
- Priority: P0 correctness, P3 architecture debt
- Symptom class: long-run memory growth after deque resize
- Status: in progress (this batch)
- Work:
  - Add EBR or hazard-pointer based reclamation for old deque arrays.

### B2. Preemption redesign
- Priority: P1 liveness, P3 architecture debt
- Symptom class: non-cooperative loops reduce fairness/progress
- Status: design locked (safepoint upgrade), runtime flip deferred
- Work:
  - Revisit signal/timer preemption path and explicit scheduler safepoints.

### B3. Formal coroutine state machine
- Priority: P0 correctness, P3 architecture debt
- Symptom class: hard-to-reason race transitions
- Status: in progress (this batch)
- Work:
  - Centralize state transitions and invariants.
  - Add transition assertions and model checks where feasible.

### B4. Steal policy optimization
- Priority: P2 performance
- Symptom class: idle-worker inefficiency, cache locality misses
- Status: deferred
- Work:
  - Evaluate global queue + local deque hybrid and topology-aware stealing.

## B2 Safepoint Design (Locked, No Behavior Flip In This Batch)
- Compiler inserts cooperative safepoints at loop backedges and function prologues.
- Runtime keeps a per-worker budget counter hook; budget expiration marks a coroutine as preemption-eligible at the next safepoint.
- This batch deliberately avoids async forced signal preemption changes. Safepoints are the upgrade path because they are auditable, deterministic, and easier to validate across platforms.

## Validation Gates (for Phase A)
- Repro case (`sample/main.ty`) completes and prints `Result:` with exit code 0.
- Repeat run loop (50-100 iterations) has zero hangs/crashes.
- Stress scenario with high `conc`/chan load completes under timeout and returns `active_coros` to 0.
- Windows runtime build remains green.
