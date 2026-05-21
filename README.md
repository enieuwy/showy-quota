# showy-bar

Always-on AI plan quota strips for **[SketchyBar](https://github.com/FelixKratz/SketchyBar)**, **[Zellij](https://github.com/zellij-org/zellij)**, and **[tmux](https://github.com/tmux/tmux)**,
driven by [CodexBar](https://github.com/steipete/CodexBar).

Beautiful, themeable, minimal.

<p align="center">
  <img src="docs/images/hero-desktop.png" alt="showy-bar running across SketchyBar and a Zellij terminal on macOS" width="960">
</p>

<p align="center"><sub>showy-bar running across SketchyBar and a Zellij terminal on macOS</sub></p>

<br>

<p align="center">
  <img src="docs/images/hero-termius.png" alt="showy-bar zellij strip running inside Termius on iPhone 16 Pro, showing four AI provider countdowns" width="720">
</p>

<p align="center"><sub>showy-bar's Zellij strip on an iPhone — four AI providers, real quotas, mid-session</sub></p>

---

```
codexbar serve → http://127.0.0.1:8080/usage
       │
       ▼
bin/showy-bar-fetch     ←  shared cache + flock + last-known-good
       │  ~/.cache/showy-bar/usage.json
       ├──► bin/showy-bar-state                 (stable provider/layout state JSON)
       ├──► sketchybar/plugins/showy_bar.sh    (native SketchyBar rows + icons)
       ├──► bin/showy-bar-zellij-bar           (ANSI strip for zjstatus pipe)
       └──► bin/showy-bar-tmux-bar             (tmux #[…] markup for status-right)
```

### Features
- **Zero auth/config:** Relies entirely on CodexBar for credentials and parsing.
- **Provider status (SketchyBar):** Icons automatically tint yellow (minor/maintenance) or red (major/critical) during an outage. Clicking a degraded icon opens the provider's official status page.
- **Pacing & thresholds:** Renders proportional pacing markers where the surface supports them and color-codes usage (good/warn/bad) based on configurable remaining-quota and time thresholds.
- **Themeable:** Ships with Catppuccin, Nord, Dracula, Tokyo Night, and others.
- **Low overhead:** A single cached fetcher serves every running bar.

## Quickstart

1. **Install and enable CodexBar.** It is the only thing that talks to providers.

   ```sh
   brew install --cask steipete/tap/codexbar          # macOS
   # CLI tarball / Linux: https://github.com/steipete/CodexBar/releases
   codexbar usage --format json | jq length           # should print 1 or more
   ```

   On macOS, cookie-based providers also need Full Disk Access for CodexBar in
   **System Settings → Privacy & Security**. If `jq length` prints `0`, fix
   CodexBar before continuing — showy-bar has nothing to paint without it.

2. **Install showy-bar.**

   ```sh
   git clone https://github.com/enieuwy/showy-bar && cd showy-bar
   make doctor                    # bash 4+, jq, codexbar present
   make install                   # symlinks bin/* into ~/.local/bin
   ```

   `make install` refuses to clobber existing files unless you run with
   `FORCE=1`, and it does **not** wire any UI. Each bar integration is opt-in.

3. **Wire a UI.** Pick the UI(s) you use:

   - **SketchyBar:** `make install-sketchybar`, then add
     `source "$ITEM_DIR/showy_bar.sh"` to your `sketchybarrc` and reload.
   - **tmux:** paste the snippet in [tmux wiring](#tmux-wiring) into `~/.tmux.conf`.
   - **Zellij:** install `zjstatus.wasm`, paste the layout fragment, start
     `showy-bar-zellij-pipe`. See [`docs/zellij.md`](docs/zellij.md).

### SketchyBar wiring

Install the SketchyBar item/plugin, then add the item declaration to
`~/.config/sketchybar/sketchybarrc` after `ITEM_DIR` and `PLUGIN_DIR` are
defined:

```sh
make install-sketchybar
source "$ITEM_DIR/showy_bar.sh"
```

Then reload SketchyBar (`sketchybar --reload` or quit + relaunch) once to
load the trigger item. One icon + bar + label triple appears per provider
currently fetching usage data; later provider adds/removals land on the next
plugin tick without another reload.

### Zellij wiring

Three pieces:

1. **Widget pane** — paste `zellij/layout-pane.kdl.fragment` into your
   default layout. It declares the visible `zjstatus` pipe widget only.
   Install `zjstatus.wasm` first; see [`docs/zellij.md`](docs/zellij.md).
2. **Pipe loop** — start `showy-bar-zellij-pipe` for each Zellij session
   (for example, `ZELLIJ_SESSION_NAME=test showy-bar-zellij-pipe` from the
   terminal wrapper that launches the session).
3. **Detail keybind** — paste `zellij/detail-pane.kdl.fragment` into your
   keybinds block. Default is `Alt /`.

Reload Zellij to pick up the new layout/keybind, then start the pipe loop.

### tmux wiring

```sh
# Use the absolute path — tmux's PATH at server start typically lacks ~/.local/bin.
CB_BIN="$HOME/.local/bin"
cat >> ~/.tmux.conf <<TMUX
set -g status-right-length 300
if -F '#{m:*showy-bar-tmux-bar*,#{status-right}}' '' 'set -ag status-right " #(${CB_BIN}/showy-bar-tmux-bar)"'
bind-key "/" display-popup -E -h 36 -w 92 -T "CodexBar usage" 'config="\${XDG_CONFIG_HOME:-\$HOME/.config}/showy-bar/config.env"; [ -r "\$config" ] && . "\$config"; while :; do clear; "\${SHOWY_BAR_CODEXBAR_BIN:-codexbar}" usage; sleep 30; done'
TMUX
tmux source ~/.tmux.conf
```

No `watch(1)` dependency — the popup uses a tiny shell loop so this works on a
stock macOS install.

### Terminal rendering modes

Terminal strips default to an `auto` mode that picks a body layout per
provider:

- **`dual`** (default for time-tier providers like 5h/7d): a
  primary-over-secondary half-block layout. The pacing marker tints one
  cell's background with the `elapsed` color; body width is 12 cells.
- **`mono3`** (default for `gemini`, `antigravity`): packs primary,
  secondary, and tertiary into a single sextant cell per column with
  top/middle/bottom rows. Uses a single provider-level foreground color
  and inserts a light `│` pacing separator between cells (body width is
  13 when the marker is interior).

<p>
  <img src="docs/images/mono3-terminal.png" alt="mono3 terminal rendering layout" width="420">
</p>

Customize terminal layout with `SHOWY_BAR_TERMINAL_BAR_MODE=dual|mono3|sextant3`.
For `mono3` auto-mode selection and marker behavior, use
`SHOWY_BAR_MONO3_PROVIDERS`, `SHOWY_BAR_MONO3_PROVIDERS_EXCLUDE`, and
`SHOWY_BAR_MONO3_MARKER_SOURCE`.

Stuck? `bin/showy-bar --diagnose` (or `make diagnose`) prints exactly the
state a bug report needs.

## Requirements

- **macOS** for SketchyBar. Zellij/tmux bars also work on Linux when CodexBar
  can fetch your chosen providers.
- A CodexBar data source:
  - preferred: `codexbar serve` reachable at `http://127.0.0.1:8080/usage`
    (requires `curl`);
  - fallback: `codexbar` CLI on PATH.
  CodexBar's web-backed providers remain macOS-only; CLI/OAuth/API/local
  providers work where CodexBar supports them.
- `bash` 4+ (macOS users usually need Homebrew `bash`), `jq`, and a `date`
  that understands either `-j -f` (BSD/macOS) or `-d` (GNU coreutils).
- SketchyBar integration also needs `sketchybar` on the PATH. Font icon mode
  needs `sketchybar-app-font`; SVG fallback icons need ImageMagick 7+
  (`magick`). Native usage rows do not need `magick`.
- The Zellij renderer wraps each provider chunk in Powerline-Extra end
  caps (U+E0B6 / U+E0B4). Any Nerd Font ships these; with a non-Nerd
  font, set `SHOWY_BAR_CAP_LEFT=` / `SHOWY_BAR_CAP_RIGHT=` to blank
  them. Terminal sextant modes have additional font notes in `docs/zellij.md`
  and `docs/tmux.md`.
- Optional: `flock` for inter-process locking; falls back to an owner-scoped
  `mkdir` lock when missing.

## Configuration

Every script reads optional overrides from `~/.config/showy-bar/config.env`.
The file is optional; create it only for values you want to override.

Most users only need these; the full environment surface lives in
[`share/config.env.example`](share/config.env.example).

| Variable | Default | Effect |
|---|---|---|
| `SHOWY_BAR_THEME` | unset (default palette) | Load a named built-in or user palette. |
| `SHOWY_BAR_PROVIDERS` | empty | Ordered provider allow-list; empty renders CodexBar's enabled providers. |
| `SHOWY_BAR_PROVIDERS_EXCLUDE` | empty | Provider deny-list applied after the allow-list. |
| `SHOWY_BAR_PROVIDER_ORDER` | `codex,claude,opencode,gemini` | Stable render order without filtering. |
| `SHOWY_BAR_REFRESH_SECONDS` | `120` | Slow CLI fallback refresh interval. |
| `SHOWY_BAR_CODEXBAR_SERVE_URL` | `http://127.0.0.1:8080` | Local `codexbar serve` base URL; set empty to skip HTTP probing. |
| `SHOWY_BAR_CODEXBAR_SERVE_REFRESH_SECONDS` | `10` | Refresh interval when `codexbar serve` is available. |
| `SHOWY_BAR_TIME_WARN_MINUTES` | `30` | Urgent countdown threshold. |
| `SHOWY_BAR_SKETCHYBAR_CLICK` | `open -b com.steipete.codexbar` | Default SketchyBar click action; degraded icons open provider status URLs. |

Palette overrides use role-first keys such as `SHOWY_BAR_PALETTE_PRIMARY_*`.
Secondary and tertiary row colors auto-derive from the primary palette at
`0.55` unless overridden; see `share/config.env.example` for the full palette
surface.

## Theme gallery

| theme name | SketchyBar image | terminal / Zellij image |
|---|---|---|
| `carbonfox` | <img src="docs/images/themes/carbonfox-sketchybar.svg" alt="carbonfox SketchyBar preview" width="420"> | <img src="docs/images/themes/carbonfox-terminal.png" alt="carbonfox terminal preview" width="420"> |
| `catppuccin-frappe` | <img src="docs/images/themes/catppuccin-frappe-sketchybar.svg" alt="catppuccin-frappe SketchyBar preview" width="420"> | <img src="docs/images/themes/catppuccin-frappe-terminal.png" alt="catppuccin-frappe terminal preview" width="420"> |
| `catppuccin-latte` | <img src="docs/images/themes/catppuccin-latte-sketchybar.svg" alt="catppuccin-latte SketchyBar preview" width="420"> | <img src="docs/images/themes/catppuccin-latte-terminal.png" alt="catppuccin-latte terminal preview" width="420"> |
| `catppuccin-macchiato` | <img src="docs/images/themes/catppuccin-macchiato-sketchybar.svg" alt="catppuccin-macchiato SketchyBar preview" width="420"> | <img src="docs/images/themes/catppuccin-macchiato-terminal.png" alt="catppuccin-macchiato terminal preview" width="420"> |
| `catppuccin-mocha` | <img src="docs/images/themes/catppuccin-mocha-sketchybar.svg" alt="catppuccin-mocha SketchyBar preview" width="420"> | <img src="docs/images/themes/catppuccin-mocha-terminal.png" alt="catppuccin-mocha terminal preview" width="420"> |
| `catppuccin-mocha-blue` | <img src="docs/images/themes/catppuccin-mocha-blue-sketchybar.svg" alt="catppuccin-mocha-blue SketchyBar preview" width="420"> | <img src="docs/images/themes/catppuccin-mocha-blue-terminal.png" alt="catppuccin-mocha-blue terminal preview" width="420"> |
| `default` | <img src="docs/images/themes/default-sketchybar.svg" alt="default SketchyBar preview" width="420"> | <img src="docs/images/themes/default-terminal.png" alt="default terminal preview" width="420"> |
| `dracula` | <img src="docs/images/themes/dracula-sketchybar.svg" alt="dracula SketchyBar preview" width="420"> | <img src="docs/images/themes/dracula-terminal.png" alt="dracula terminal preview" width="420"> |
| `gruvbox-dark` | <img src="docs/images/themes/gruvbox-dark-sketchybar.svg" alt="gruvbox-dark SketchyBar preview" width="420"> | <img src="docs/images/themes/gruvbox-dark-terminal.png" alt="gruvbox-dark terminal preview" width="420"> |
| `nord` | <img src="docs/images/themes/nord-sketchybar.svg" alt="nord SketchyBar preview" width="420"> | <img src="docs/images/themes/nord-terminal.png" alt="nord terminal preview" width="420"> |
| `tokyonight` | <img src="docs/images/themes/tokyonight-sketchybar.svg" alt="tokyonight SketchyBar preview" width="420"> | <img src="docs/images/themes/tokyonight-terminal.png" alt="tokyonight terminal preview" width="420"> |

## Verification

```sh
make doctor      # check runtime prerequisites
make test        # smoke tests over JSON fixtures
make diagnose    # printable bug-report state
```

Cache lives at `${XDG_CACHE_HOME:-~/.cache}/showy-bar/usage.json`.
`make clean` clears it.

## How it stays cheap

- One shared fetcher serves every running bar, preferring `codexbar serve` and
  falling back to the CLI.
- Bars never blank on transient `codexbar` failure: the fetcher serves the
  last-known-good cache.

## License

[MIT](LICENSE) — same as CodexBar.

## Credits

[CodexBar](https://github.com/steipete/CodexBar) by Peter Steinberger does
all the real work. This repo just paints its output onto status bars.
