# Automation & prompts

`showy-quota` ships two automation-friendly subcommands that read the **same
provider metrics every bar renderer uses** (via `showy-quota-state --json`), so
they never re-parse CodexBar's JSON:

- **`showy-quota guard`** — a threshold gate for CI, cron, and agent hooks. It
  refreshes the cache, evaluates provider quota, and returns a stable exit code.
- **`showy-quota prompt`** — a one-line segment for shell prompts (starship,
  powerlevel10k, plain `PS1`). It reads the cache as-is and never blocks.

Both rely on the shared fetch/cache plumbing. A local status bar, the Zellij
pipe, or a timer normally keeps the cache warm; see the wiring docs for those.

---

## `showy-quota guard`

```
showy-quota guard [--provider ID[,ID...]] [--window primary|secondary|tertiary|worst]
                  [--min-remaining PCT | --max-used PCT] [--allow-stale]
                  [--wait-max SECONDS] [--json] [--quiet]
```

Guard runs a fetch first (the normal refresh-window semantics — a fetch failure
is **not** fatal when a cache already exists), then evaluates
`showy-quota-state` provider metrics and exits with a stable code.

### Flags

| Flag | Meaning |
|---|---|
| `--provider ID[,ID...]` | Restrict evaluation to these provider ids. Empty = every renderable provider. Each named provider must be usable (see exit `2`). |
| `--window WIN` | Which usage window to test: `primary`, `secondary`, `tertiary`, or `worst` (default). `worst` = the non-null slot with the least remaining, per provider. An explicit window that is null for a selected provider is unusable data. |
| `--min-remaining PCT` | Fail when remaining quota drops below `PCT`. Default when no threshold flag is given: `--min-remaining 10`. |
| `--max-used PCT` | Fail when used quota rises above `PCT`. Mutually exclusive with `--min-remaining` (giving both is a usage error). |
| `--allow-stale` | Evaluate even when the cache is stale. Without it, a stale cache is unusable data. |
| `--wait-max SECONDS` | On a breach whose worst window has a known reset within `SECONDS`, sleep until reset (+30s grace), force one refresh, and re-evaluate once. Otherwise fail immediately. |
| `--json` | Emit one machine-readable object (see below) on every non-usage outcome. |
| `--quiet` | Print nothing on pass; one human line on failure (unless `--json`). |

### Threshold boundary (inclusive-pass)

Breaches are **strict**, so a value exactly at the threshold **passes**:

- `--min-remaining N` → pass when `remaining >= N`, breach when `remaining < N`.
- `--max-used N` → pass when `used <= N`, breach when `used > N`.

For example, a provider window sitting at exactly 95% remaining **passes**
`--min-remaining 95` and **breaches** `--min-remaining 96`.

### Exit codes

| Code | Meaning | Examples |
|---|---|---|
| `0` | Pass — no selected provider breaches the threshold. | quota healthy |
| `1` | Breach — at least one selected provider is over the limit (the worst is reported). | remaining below `--min-remaining` |
| `2` | Unusable data — the request could not be evaluated. | no/empty cache; unknown provider; a selected provider errored; a stale cache without `--allow-stale`; an explicit window that is null |
| `3` | Usage error — bad flags. | `--min-remaining` and `--max-used` together; a non-integer or out-of-range percentage; an unknown flag |

Only exit `3` prints nothing machine-readable; every other outcome (`0`/`1`/`2`)
emits the `--json` object when `--json` is set.

### `--json` output

One object on stdout for exit `0`, `1`, and `2`:

```json
{
  "exit": 1,
  "ok": false,
  "reason": "breach",
  "stale": false,
  "evaluated": 1,
  "worst": {
    "provider": "codex",
    "window": "primary",
    "usedPercent": 96,
    "remainingPercent": 4,
    "minutesUntilReset": 182,
    "resetsAt": "2026-01-01T03:02:00Z"
  }
}
```

`worst` is `null` when nothing was evaluable (e.g. `reason: "no-cache"`).
`reason` is one of `pass`, `breach`, `no-cache`, `unknown-provider: …`,
`provider-error: <id>(<kind>),…`, `null-window: <id>/<window>`,
`no-usable-data`, or `stale`.

### Examples

**CI pre-flight** — gate a workflow step so a job does not start on a nearly
exhausted budget:

```yaml
# .github/workflows/ci.yml
- name: Check AI quota
  run: showy-quota guard --provider codex,claude --min-remaining 15
```

**Claude Code `PreToolUse` hook** — block tool calls while quota is low. Claude
Code treats hook **exit code 2** as "block"; guard reports a breach as exit `1`,
so the one-liner re-maps only a breach to `2` and lets missing data (exit `2`
from guard) or usage errors pass through without blocking:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "showy-quota guard --provider codex --min-remaining 15 --quiet; [ \"$?\" -eq 1 ] && exit 2 || exit 0"
          }
        ]
      }
    ]
  }
}
```

**Cron alert** — notify (macOS example) when the worst window drops under 10%:

```cron
*/30 * * * * /usr/local/bin/showy-quota guard --min-remaining 10 --quiet || \
  /usr/bin/osascript -e 'display notification "AI quota low" with title "showy-quota"'
```

**Wait for reset** — in a long batch, pause until the breaching window resets
(up to an hour) instead of bailing:

```sh
showy-quota guard --provider codex --min-remaining 5 --wait-max 3600 || {
  echo "codex quota still exhausted after waiting" >&2
  exit 1
}
```

---

## `showy-quota prompt`

```
showy-quota prompt [--ansi] [--provider ID]
```

Prints one line with no trailing whitespace for the **worst-remaining**
provider window:

```
CX 92% 3:02
```

`<SIGIL> <used>% <countdown>` — the two/three-letter provider sigil, the used
percentage, and a `H:MM`-style countdown to reset (matching the strips; omitted
when the reset time is unknown). `--provider ID` restricts the segment to one
provider. With no data (empty cache) it prints `AI ?`.

- `--ansi` adds a 16-color severity color: green when remaining ≥ 50, yellow
  when ≥ 20, red below 20. `NO_COLOR` suppresses color entirely.
- A stale cache appends the configured stale glyph (`SHOWY_QUOTA_STALE_GLYPH`,
  default `⚠`).
- `prompt` **always exits 0** (except on a bad flag) — a prompt segment must
  never break a shell.

### Fast by default (no fetch)

`prompt` reads the cache **as-is** (`showy-quota-state --no-fetch`) and never
performs a network refresh. This keeps it well under the ~500ms budget prompt
frameworks allow (starship's `command_timeout` defaults to `500`). A cold
CodexBar collection can take seconds, so refreshing is opt-in:

- Default: read the cache. Keep it warm with a status bar, the Zellij pipe, or a
  timer (`showy-quota-fetch` on a schedule).
- `SHOWY_QUOTA_PROMPT_FETCH=1`: run the normal refresh-window fetch before
  reading (only sensible where nothing else keeps the cache warm).

### starship

`showy-quota prompt` is a plain, fast command — a good fit for a starship
[custom command module](https://starship.rs/config/#custom-commands). Let
starship style the output via `$style`:

```toml
# ~/.config/starship.toml
[custom.showy_quota]
command = 'showy-quota prompt'
when = true
shell = ['bash', '--noprofile', '--norc']
format = '[$output]($style) '
style = 'bold yellow'
description = 'Worst AI provider quota'
```

Add `${custom.showy_quota}` (or `$custom`) to your `format`/`right_format` if
you use an explicit prompt format. To let `showy-quota` colorize instead of
starship, use `command = 'showy-quota prompt --ansi'` together with
`unsafe_no_escape = true` so the ANSI codes are not escaped.

### powerlevel10k

Define a [custom segment](https://github.com/romkatv/powerlevel10k#extensibility)
function and add its name to your prompt elements. Put this in
`~/.p10k.zsh` (or after sourcing p10k):

```zsh
function prompt_showy_quota() {
  local q
  q="$(showy-quota prompt 2>/dev/null)" || return
  [[ -n $q && $q != 'AI ?' ]] || return
  p10k segment -t "$q"
}

# Add `showy_quota` to your prompt elements, e.g.:
typeset -g POWERLEVEL9K_RIGHT_PROMPT_ELEMENTS=( ... showy_quota )
# Optional styling:
typeset -g POWERLEVEL9K_SHOWY_QUOTA_FOREGROUND=3
```

### plain PS1 (bash/zsh)

```sh
# bash — recomputed each prompt via PROMPT_COMMAND
PROMPT_COMMAND='__sq=$(showy-quota prompt 2>/dev/null)'
PS1='${__sq:+$__sq }\u@\h:\w\$ '
```

```zsh
# zsh — enable parameter expansion in the prompt
setopt prompt_subst
PS1='$(showy-quota prompt 2>/dev/null) %n@%m:%~%# '
```
