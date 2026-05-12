# showy-bar

Always-on AI coding-quota strips for **SketchyBar**, **Zellij**, and **tmux**,
driven entirely by the [CodexBar](https://github.com/steipete/CodexBar) CLI.

CodexBar handles every provider's auth, cookies, OAuth, parsing, and caching.
This repo's only job is to render its JSON in three places:

```
codexbar usage --format json
       │
       ▼
bin/showy-bar-fetch     ←  shared cache + flock + last-known-good
       │  ~/.cache/showy-bar/usage.json
       ├──► bin/showy-bar-state                 (stable provider/layout state JSON)
       ├──► sketchybar/plugins/showy_bar.sh    (per-provider PNG icon + bar)
       ├──► bin/showy-bar-zellij-bar           (ANSI strip for zjstatus pipe)
       └──► bin/showy-bar-tmux-bar             (tmux #[…] markup for status-right)
```

No sidecar, no daemon, no provider auth code, no extra config beyond a
single optional env file.

## Requirements

- **macOS** for SketchyBar. Zellij/tmux bars also work on Linux when the
  CodexBar CLI can fetch your chosen providers.
- [`codexbar`](https://github.com/steipete/CodexBar) on the PATH and
  configured.
  - macOS app: `brew install --cask steipete/tap/codexbar`, then enable
    providers in **CodexBar → Preferences → Providers**.
  - CLI tarballs / Linux: install the CodexBar CLI from GitHub Releases or
    `brew install steipete/tap/codexbar`, then configure `~/.codexbar/config.json`.
    CodexBar's web-backed providers remain macOS-only; CLI/OAuth/API/local
    providers work where CodexBar supports them.
- `bash` 4+, `jq`, ImageMagick 7+ (`magick`), and a `date` that understands
  either `-j -f` (BSD/macOS) or `-d` (GNU coreutils).
- Optional: `flock` for inter-process locking; falls back to an owner-scoped
  `mkdir` lock when missing.

The SketchyBar plugin sources provider icons from
`/Applications/CodexBar.app/Contents/Resources/ProviderIcon-<id>.svg`. No
icons are bundled in this repo.

## Install

```sh
cd /path/to/showy-bar
make install                   # symlinks bin/* into ~/.local/bin only
```

`make install` refuses to clobber existing files and refuses to retarget
existing symlinks unless you explicitly run with `FORCE=1`.

`make install` does **not** wire any UI by default. Each bar integration is
opt-in so tmux/Zellij users do not get SketchyBar files, and vice versa.

To uninstall:

```sh
make uninstall
```

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

Two pieces:

1. **Pipe loop** — paste `zellij/layout-pane.kdl.fragment` into your
   default layout (the `pane size=1` widget plus the tiny floating command
   pane that runs `showy-bar-zellij-pipe`). Install `zjstatus.wasm` first; see
   [`docs/zellij.md`](docs/zellij.md).
2. **Detail keybind** — paste `zellij/detail-pane.kdl.fragment` into your
   keybinds block. Default is `Alt /`.

Reload Zellij to pick up the new layout.

### tmux wiring

```sh
# Use the absolute path — tmux's PATH at server start typically lacks ~/.local/bin.
CB_BIN="$HOME/.local/bin"
printf 'set -ag status-right " #(%s/showy-bar-tmux-bar)"\n' "$CB_BIN" >> ~/.tmux.conf
printf 'bind-key "/" display-popup -E -h 36 -w 92 -T "CodexBar usage" %s\n' \
    "'while :; do clear; codexbar usage; sleep 30; done'" >> ~/.tmux.conf
tmux source ~/.tmux.conf
```

No `watch(1)` dependency — the popup uses a tiny shell loop so this works on a
stock macOS install.

## Configuration

Every script reads optional overrides from
`~/.config/showy-bar/config.env` (see
[`share/config.env.example`](share/config.env.example) for the full list
of variables). All values have working defaults; the file is optional.

Choose a named palette by editing `~/.config/showy-bar/config.env`:

```sh
SHOWY_BAR_THEME=catppuccin-mocha-blue
```

Built-ins include Default, Carbonfox, Catppuccin variants, Dracula, Gruvbox
Dark, Nord, and Tokyo Night.

### Theme gallery

Each preview uses the same two-provider fixture with `3:29` and `23m`
countdowns, good/warn/bad remaining-usage colors, and different pacing-marker
positions visible. SketchyBar previews are composed from the plugin-generated
provider icon/bar PNGs using CodexBar's bundled provider SVG logos. Terminal
previews show the deterministic `showy-bar --preview` output.

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

Useful knobs:

| Variable                          | Default                                | Effect                                                |
|-----------------------------------|----------------------------------------|-------------------------------------------------------|
| `SHOWY_BAR_REFRESH_SECONDS`         | `120`                                  | Upper bound on how often `codexbar` itself is invoked |
| `SHOWY_BAR_PROVIDERS`               | empty (render CodexBar's enabled providers) | Comma-list allow-list, e.g. `claude,codex`            |
| `SHOWY_BAR_PROVIDERS_EXCLUDE`       | empty                                  | Comma-list exclude-list applied after the allow-list  |
| `SHOWY_BAR_INCLUDE_STATUS`          | `1`                                    | Include CodexBar status for outage-tinted logos and status-page icon clicks |
| `SHOWY_BAR_TIME_WARN_MINUTES`       | `30`                                   | Threshold for red countdown labels                    |
| `SHOWY_BAR_THEME`                   | empty                                  | Load `~/.config/showy-bar/themes/<name>.env` or the built-in `share/themes/<name>.env` palette |
| `SHOWY_BAR_PALETTE_PRIMARY_*`       | Original ai-quota palette              | Canonical role-first good/warn/bad/unknown colors     |
| `SHOWY_BAR_PALETTE_COUNTDOWN`       | `7b8496`                               | Normal Zellij and SketchyBar countdown label color    |
| `SHOWY_BAR_PALETTE_COUNTDOWN_WARN`  | primary bad color                      | Urgent countdown label color                          |
| `SHOWY_BAR_PALETTE_ICON_TEXT`       | `f2f4f8`                               | Fallback provider icon text color                     |
| `SHOWY_BAR_SKETCHYBAR_CLICK`        | `open -b com.steipete.codexbar`        | Default SketchyBar click action; degraded icons open provider status URLs |
| `SHOWY_BAR_SKETCHYBAR_PILL_*`       | `14` / `28` / `0xcc24273a`             | SketchyBar bracket radius, height, and ARGB color     |
| `SHOWY_BAR_CODEXBAR_RESOURCES`      | `/Applications/CodexBar.app/...`       | Where to find provider SVGs                           |
| `SHOWY_BAR_SKETCHYBAR_COMPACT_PROVIDER_COUNT` | `5` | Provider-count breakpoint exposed by `showy-bar-state` for external layout managers |

Secondary and tertiary row colors auto-derive from the primary palette at
`0.55` by default. Override `SHOWY_BAR_PALETTE_SECONDARY_*`,
`SHOWY_BAR_PALETTE_TERTIARY_*`, or the `*_SCALE` knobs when you want custom
per-role colors.

## Verification

```sh
make test                         # smoke tests over JSON fixtures
bin/showy-bar-fetch | jq length     # 1+ if CodexBar has providers enabled
bin/showy-bar-state                  # JSON state for layout managers
bin/showy-bar --list            # available palette themes
bin/showy-bar --preview default # deterministic ANSI theme preview
bin/showy-bar-zellij-bar            # ANSI strip
bin/showy-bar-tmux-bar              # tmux markup
```

Cache lives at `${XDG_CACHE_HOME:-~/.cache}/showy-bar/usage.json`.
`make clean` clears it.

## How it stays cheap

- One `codexbar` invocation per `SHOWY_BAR_REFRESH_SECONDS` regardless of how
  many bars are running.
- SketchyBar compares the desired provider set to its last declared state once
  per tick; add/remove and bracket rebuild only happen when that set changes.
- SketchyBar's plugin only writes a PNG when its bytes change (atomic
  `cmp`-then-`mv`).
- Provider icon PNGs are generated once per provider per cache directory.
- Bars never blank on transient `codexbar` failure: the fetcher serves
  the last-known-good cache and exits 0.

## Limitations

- `codexbar` runs from a GUI macOS app bundle; cookie-based providers
  need Full Disk Access in System Settings → Privacy & Security to
  decrypt browser cookies.
- The strip omits CodexBar's `tertiary` window for tmux/Zellij. Zellij shows
  primary over secondary in a single half-block strip; tmux remains primary-only
  with the compact secondary hint.
  SketchyBar shows up to three stacked bars when the provider exposes a
  tertiary window.
- **Stale-cache dimming is terminal-only.** When the cache is older than
  `2 × SHOWY_BAR_REFRESH_SECONDS`, the Zellij and tmux strips dim every
  provider chunk. SketchyBar continues to render at full strength —
  CodexBar's own menu icon will reflect upstream incidents.
- No Linux-side provider for browser-cookie providers — same constraint
  as CodexBar itself.

## Layout

```
bin/             showy-bar-fetch, showy-bar-state, showy-bar,
                 showy-bar-{zellij,tmux}-bar, showy-bar-zellij-pipe
lib/             common.sh, strip.sh
sketchybar/      items/showy_bar.sh, plugins/showy_bar.sh
zellij/          fragments
tmux/            fragments
share/           config.env.example
test/            render_test.sh + JSON fixtures
docs/            architecture.md
```

## License

[MIT](LICENSE) — same as CodexBar.

## Credits

[CodexBar](https://github.com/steipete/CodexBar) by Peter Steinberger does
all the real work. This repo just paints its output onto status bars.
