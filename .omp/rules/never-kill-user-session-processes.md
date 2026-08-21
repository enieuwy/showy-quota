---
name: never-kill-user-session-processes
description: "Running Zellij panes/plugins and session-owned codexbar processes are user state — never kill or close them to clear a diagnosis"
condition:
  - '(pkill|killall)\b'
  - 'kill\s+(-(9|KILL|TERM|INT|HUP|QUIT|STOP)\s+)?["'']?(\$|[0-9(])'
  - 'zellij (kill-session|kill-all-sessions|delete-session|action close-pane)'
  - 'launchctl (unload|kickstart -k|bootout)[^\n]*sketchybar'
scope: ["tool:bash"]
interruptMode: always
---

**Running Zellij panes/plugins are user state.** Do not close panes, kill plugin panes, or kill session-owned `codexbar serve` / `codexbar usage` processes without first stating exact intent and expected impact — and getting agreement.

This repo's whole hot path is a lens over *someone's live session*. Killing a `codexbar serve` the user's session owns does not just end a process: every surface reading the shared cache degrades, the plugin falls back, and you have destroyed the state you were trying to observe. Worse, a `pkill codexbar` cannot tell a session-owned server from one this repo's own tests started.

Also: an active Zellij session can retain stale layout/plugin configuration. When that is the problem, **explain that a restart, reload, or new tab is needed** — do not forcibly rearrange the session to make the symptom go away.

Non-destructive ways to get the same information:

```bash
make diagnose                 # runtime state for bug reports
showy-quota-state             # filtered provider/layout JSON
pgrep -fl codexbar            # observe without signalling
ls -l "${SHOWY_QUOTA_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/showy-quota}"
kill -0 <pid>                 # liveness probe only, sends nothing
```

If a process you started in *this* task has to go, name it, name its pid, and say why it is yours.
