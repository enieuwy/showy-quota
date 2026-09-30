---
name: no-history-or-forecasting-judged
description: "Judged companion to no-history-or-forecasting: catches stored history, sampling, or trend math under any identifier"
scope:
  - "tool:edit(bin/*)"
  - "tool:write(bin/*)"
  - "tool:edit(lib/*.sh)"
  - "tool:write(lib/*.sh)"
  - "tool:edit(crates/**/*.rs)"
  - "tool:write(crates/**/*.rs)"
question: "Does this edit add stored usage history, periodic sampling, a time series, or trend, burn-rate, or forecast calculation, rather than rendering the current quota state?"
---

**`showy-quota` is a current-state quota view, not a store.** Usage history,
sampling, burn-rate, trend, and forecasting are explicit non-goals. They need
periodic wakeups and growing state, which is the opposite of lightweight.

In-scope moves: render the current quota, expose it as JSON
(`showy-quota-state`), gate on it (`showy-quota guard`), and move per-render
work into the native binary.

If the user asked for this, or you are building a downstream consumer, say so
and continue. Full rationale: `rule://no-history-or-forecasting`.
