---
name: no-history-or-forecasting
description: "Usage history, time-series, burn-rate/forecasting and analytics are explicit non-goals — they imply periodic wakeups and growing state"
condition:
  - 'burn.?rate'
  - 'forecast'
  - 'time.?series'
  - 'trend(line|ing)?_?(data|series|calc)'
  - 'history_(store|buffer|series|samples)'
scope:
  - "tool:edit(bin/*)"
  - "tool:write(bin/*)"
  - "tool:edit(lib/*.sh)"
  - "tool:write(lib/*.sh)"
  - "tool:edit(crates/**/*.rs)"
  - "tool:write(crates/**/*.rs)"
interruptMode: always
---

**`showy-quota` is a lightweight, current-state quota *view* — a lens over CodexBar data, not a store.** A status bar answers "where am I now," not "where was I."

Explicit **non-goals**: usage history, time-series/sampling, burn-rate/trend/forecasting, analytics dashboards. The reason is not taste, it is cost: these imply **periodic wakeups and growing state** — the opposite of lightweight — and belong in a downstream system. Energy here is dominated by process spawns and wake frequency, not raw wall-time, so a sampler is expensive even when it is fast. Provider ownership likewise stays with CodexBar (allow/exclude/order only): showy-quota is a lens, not a data owner.

There is currently almost nothing matching these identifiers in `bin/`, `lib/`, `crates/`, or `adapters/` — this is a clean boundary, not a cleanup.

If the goal is a better *current-state* answer, the in-scope moves are: render the current quota, expose it as JSON (`showy-quota-state`), gate on it (`showy-quota guard`), and push per-render work into the single native binary rather than adding shell/`jq`/`date` scaffolding to the hot path. Prefer event-driven or coalesced refresh over tight polling, and keep the shared-cache single-refresher model so N surfaces do not each poll.

If the user is deliberately building a downstream consumer, or asked for this explicitly, say so and continue.
