# Agent CLI statusline

Put the quota strip inside the agent CLI that is *spending* the quota. When you
code with Claude Code, Codex, opencode, or any similar TUI, its status line can
render the same showy-quota strip you already use in Zellij/tmux, so the
countdown to your next reset is in front of you exactly where it matters —
without a separate bar, pane, or window.

`adapters/agent-cli/showy-quota-statusline` is a thin wrapper around
`bin/showy-quota-zellij-bar`. It:

- drains stdin (Claude Code and compatible harnesses pipe a session-JSON blob
  there; the adapter reads and discards it),
- resolves `showy-quota-zellij-bar` — `SHOWY_QUOTA_BAR_BIN` override first, then
  the sibling `../../bin`, then `PATH`,
- applies statusline-flavoured defaults (see [Environment knobs](#environment-knobs)),
- and execs the bar. Whatever ANSI strip the bar prints is what the agent CLI
  displays.

It never hard-fails: if the bar cannot be found it prints `AI ?` and exits 0,
because a status-line segment must never break the harness that runs it.

## Output shape

The adapter emits the same styled one-line strip as the Zellij/zjstatus driver,
just narrower — rounded end caps and all (set `SHOWY_QUOTA_STATUSLINE_CAPS=0`
for plain-font hosts, which yields the capless form):

```text
CX▕▀▀▀▀▀▀▀▀▏1:23 CL▕▀▀▀▀▀▀▀▀▏1:08
```

Each provider chunk is `<SIGIL>▕<bar body>▏<countdown>`; see
[`docs/zellij.md`](zellij.md#output-shape) for the body geometry, severity
colors, stale/degraded markers, and terminal-mode knobs, all of which the
statusline inherits.

## Claude Code

Claude Code runs any command you configure as its status line, pipes session
JSON to it on stdin, and renders the first line(s) it prints (ANSI colors
supported). Add a `statusLine` block to `~/.claude/settings.json` (or a project
`.claude/settings.json`), set `type` to `"command"`, and point `command` at the
adapter:

```json
{
  "statusLine": {
    "type": "command",
    "command": "/absolute/path/to/showy-quota/adapters/agent-cli/showy-quota-statusline",
    "refreshInterval": 30
  }
}
```

- `type` must be `"command"` and `command` is a script path or inline shell
  command (`~` is expanded in this field). Point it at the adapter in your
  checkout, or at the copied install tree (`make install-copy` ships it to
  `~/.local/share/showy-quota/adapters/agent-cli/showy-quota-statusline`, next
  to the `bin/` it resolves).
- `refreshInterval` (optional, minimum `1`, seconds) re-runs the adapter on a
  timer in addition to Claude Code's event-driven updates. Set it so the reset
  countdown keeps ticking while the session is idle; `30` is a good balance
  against the cache refresh window.
- Settings reload automatically, but a change first shows on your next
  interaction with Claude Code.

Verified against Claude Code's [statusline docs](https://code.claude.com/docs/en/statusline).

## Codex CLI, opencode, and other TUIs

As of this writing, neither Codex CLI nor opencode ships an external,
command-backed status line:

- **Codex CLI** has a built-in `/statusline` picker that toggles a fixed set of
  native items (model, context, rate limits, git, …) persisted to
  `~/.codex/config.toml`; it does not run an external command. External
  command-backed status lines were proposed (openai/codex PRs #10170 and #10546)
  but not merged, so there is nothing showy-quota can hook into yet. Track the
  upstream issues for command-backed rendering; if/when it lands the config will
  be a `[tui] status_line = ["…"]`-style command array and this adapter will
  drop straight in.
- **opencode** does not yet expose a shell-script status line either; it is a
  requested feature upstream.

Until those land, the adapter still works with **any tool that displays a
command's one-line ANSI output**. The adapter reads and ignores stdin, so it is
safe to run with no input (`</dev/null`), and it prints exactly one line. If
your agent runs inside tmux or Zellij, use the existing
[tmux](tmux.md)/[Zellij](zellij.md) integrations instead. Otherwise, wire the
adapter into whatever status/prompt mechanism your TUI offers that can shell out
and show the resulting line.

## Environment knobs

The statusline adapter reads two of its own knobs and passes everything else
through to the bar. Set them in the environment your agent CLI inherits (your
shell profile, or `~/.config/showy-quota/config.env`, which the bar sources):

| Variable | Default | Meaning |
|---|---|---|
| `SHOWY_QUOTA_STATUSLINE_WIDTH` | `8` | Bar body width in terminal cells. Narrower than the 12-cell terminal strip because status lines are cramped. Feeds `SHOWY_QUOTA_ZELLIJ_BAR_WIDTH`. |
| `SHOWY_QUOTA_STATUSLINE_CAPS` | `1` | `0` drops the Powerline-Extra rounded end caps (U+E0B6 / U+E0B4) on this surface only. Caps are inherited from the bar's defaults / your `SHOWY_QUOTA_CAP_*` config, matching the Zellij and tmux strips; opt out when the agent CLI renders in a font without Nerd Font glyphs, where the caps show as tofu. |
| `SHOWY_QUOTA_BAR_BIN` | — | Override the resolved `showy-quota-zellij-bar` path (absolute path or a command name on `PATH`). |

Precedence: an explicit `SHOWY_QUOTA_ZELLIJ_BAR_WIDTH` or `SHOWY_QUOTA_CAP_LEFT` /
`SHOWY_QUOTA_CAP_RIGHT` in your environment or `config.env` still wins on every
surface, including the statusline. The `SHOWY_QUOTA_STATUSLINE_*` knobs set the
statusline default only when you have not pinned those global values.

All of the shared rendering knobs — theme, palette, `SHOWY_QUOTA_TERMINAL_BAR_MODE`,
`SHOWY_QUOTA_PROVIDER_MODES`, and so on — apply here too; see
[Configuration](../README.md#configuration).

## Latency

Rendering is cache-first, so hot invocations are cheap:

- The native `showy-quota-render` binary formats a cached snapshot in well under
  a millisecond.
- The wrapped `showy-quota-fetch` respects the refresh window
  (`SHOWY_QUOTA_REFRESH_SECONDS`, default 120s): within the window it returns the
  cached snapshot without any network call. Only after the window elapses does
  one invocation refresh from `codexbar serve` (or the CLI fallback).

The remaining cost is shell process startup across the wrapper → fetch → render
chain (roughly a couple hundred milliseconds on an Apple M1 Max), not I/O. That
is well within a status-line refresh budget, and because most renders are cached
reads, they never block on the network. If you set a `refreshInterval`, keep it
comfortably above `1` so repeated renders stay on the cached path.
