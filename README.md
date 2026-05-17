# showy-bar

<p align="center">
  <img src="docs/images/hero-desktop.png" alt="showy-bar running across SketchyBar and a Zellij terminal on macOS" width="960">
</p>

<p align="center">
  <img src="docs/images/hero-termius.png" alt="showy-bar zellij strip running inside Termius on iPhone 16 Pro, showing four AI provider countdowns" width="720">
</p>

<p align="center"><sub>showy-bar's Zellij strip on an iPhone — four AI providers, real quotas, mid-session.</sub></p>

Always-on AI coding-quota strips for **SketchyBar**, **Zellij**, and **tmux**,
driven by [CodexBar](https://github.com/steipete/CodexBar) CLI JSON or its
localhost `codexbar serve` endpoint.

CodexBar handles every provider's auth, cookies, OAuth, parsing, and caching.
This repo's only job is to render its JSON in three places:

```
codexbar usage --format json     or     codexbar serve → http://127.0.0.1:8080/usage
       │
       ▼
bin/showy-bar-fetch     ←  shared cache + flock + last-known-good
       │  ~/.cache/showy-bar/usage.json
       ├──► bin/showy-bar-state                 (stable provider/layout state JSON)
       ├──► sketchybar/plugins/showy_bar.sh    (native SketchyBar rows + icons)
       ├──► bin/showy-bar-zellij-bar           (ANSI strip for zjstatus pipe)
       └──► bin/showy-bar-tmux-bar             (tmux #[…] markup for status-right)
```

No provider auth code, no extra config beyond a single optional env file. By
default, showy-bar probes `http://127.0.0.1:8080/usage` for `codexbar serve`
and falls back to spawning the CLI on refresh.

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
   make doctor                                        # verifies bash 4+, jq, codexbar
   make install                                       # symlinks bin/* into ~/.local/bin
   ```

3. **Wire exactly one UI.** `make install` does not put anything on a bar.
   Pick the one you use:

   - **SketchyBar:** `make install-sketchybar`, then add
     `source "$ITEM_DIR/showy_bar.sh"` to your `sketchybarrc` and reload.
   - **tmux:** paste the snippet in [tmux wiring](#tmux-wiring) into `~/.tmux.conf`.
   - **Zellij:** install `zjstatus.wasm`, paste the layout fragment, start
     `showy-bar-zellij-pipe`. See [`docs/zellij.md`](docs/zellij.md).

Stuck? `bin/showy-bar --diagnose` (or `make diagnose`) prints exactly the
state a bug report needs.

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
- `bash` 4+ (macOS users usually need Homebrew `bash`), `jq`, and a `date`
  that understands either `-j -f` (BSD/macOS) or `-d` (GNU coreutils).
- SketchyBar integration also needs `sketchybar` on the PATH. Font icon mode
  needs `sketchybar-app-font`; SVG fallback icons need ImageMagick 7+
  (`magick`). Native usage rows do not need `magick`.
- The Zellij renderer wraps each provider chunk in Powerline-Extra end
  caps (U+E0B6 / U+E0B4). Any Nerd Font ships these; with a non-Nerd
  font, set `SHOWY_BAR_CAP_LEFT=` / `SHOWY_BAR_CAP_RIGHT=` to blank
  them. tmux uses only Unicode Block Elements and needs no special font.
- Optional: `flock` for inter-process locking; falls back to an owner-scoped
  `mkdir` lock when missing.
- Optional: `curl` for the default `codexbar serve` probe; without it,
  showy-bar falls back to the CLI path.

In `SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_MODE=font`, mapped providers use
`sketchybar-app-font` glyphs and the rest fall back to CodexBar's bundled SVGs
at `/Applications/CodexBar.app/Contents/Resources/ProviderIcon-<id>.svg`. No
icons are bundled in this repo.

## Install

See [Quickstart](#quickstart) for the 3-step path. Detail flags:

```sh
make doctor                    # bash 4+, jq, codexbar present
make install                   # symlinks bin/* into ~/.local/bin
make install-sketchybar        # optional; SketchyBar item + plugin only
make install-all               # both
make uninstall                 # remove every symlink this Makefile created
```

`make install` refuses to clobber existing files and refuses to retarget
existing symlinks unless you explicitly run with `FORCE=1`. It does **not**
wire any UI — each bar integration is opt-in so tmux/Zellij users do not get
SketchyBar files, and vice versa.

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
if -F '#{m:*showy-bar-tmux-bar*,#{status-right}}' '' 'set -ag status-right " #(${CB_BIN}/showy-bar-tmux-bar)"'
bind-key "/" display-popup -E -h 36 -w 92 -T "CodexBar usage" 'config="\${XDG_CONFIG_HOME:-\$HOME/.config}/showy-bar/config.env"; [ -r "\$config" ] && . "\$config"; while :; do clear; "\${SHOWY_BAR_CODEXBAR_BIN:-codexbar}" usage; sleep 30; done'
TMUX
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

See [Theme gallery](#theme-gallery) further down for visual comparisons.

Useful knobs:

| Variable                          | Default                                | Effect                                                |
|-----------------------------------|----------------------------------------|-------------------------------------------------------|
| `SHOWY_BAR_REFRESH_SECONDS`         | `120`                                  | Upper bound on how often `codexbar` itself is invoked |
| `SHOWY_BAR_CODEXBAR_SERVE_URL`    | `http://127.0.0.1:8080`                | Localhost base URL for `codexbar serve`; set empty to skip the HTTP probe |
| `SHOWY_BAR_CODEXBAR_SERVE_TIMEOUT_SECONDS` | `0.5`                         | HTTP timeout for the default `codexbar serve` fetch path |
| `SHOWY_BAR_PROVIDERS`               | empty (render CodexBar's enabled providers) | Ordered comma-list allow-list, e.g. `codex,claude`   |
| `SHOWY_BAR_PROVIDERS_EXCLUDE`       | empty                                  | Comma-list exclude-list applied after the allow-list  |
| `SHOWY_BAR_PROVIDER_ORDER`          | `codex,claude,opencode,gemini`         | Stable render order without filtering; missing providers are skipped |
| `SHOWY_BAR_INCLUDE_STATUS`          | `1`                                    | Include CodexBar status for outage-tinted logos and status-page icon clicks |
| `SHOWY_BAR_TIME_WARN_MINUTES`       | `30`                                   | Threshold for red countdown labels                    |
| `SHOWY_BAR_THEME`                   | empty                                  | Load `~/.config/showy-bar/themes/<name>.env` or the built-in `share/themes/<name>.env` palette |
| `SHOWY_BAR_PALETTE_PRIMARY_*`       | Original ai-quota palette              | Canonical role-first good/warn/bad/unknown colors     |
| `SHOWY_BAR_PALETTE_COUNTDOWN`       | `7b8496`                               | Normal Zellij and SketchyBar countdown label color    |
| `SHOWY_BAR_PALETTE_COUNTDOWN_WARN`  | primary bad color                      | Urgent countdown label color                          |
| `SHOWY_BAR_PALETTE_ICON_TEXT`       | `f2f4f8`                               | Fallback provider icon text color                     |
| `SHOWY_BAR_SKETCHYBAR_CLICK`        | `open -b com.steipete.codexbar`        | Default SketchyBar click action; degraded icons open provider status URLs |
| `SHOWY_BAR_SKETCHYBAR_PILL_*`       | `14` / `28` / `0xcc24273a`             | SketchyBar bracket radius, height, and ARGB color     |
| `SHOWY_BAR_SKETCHYBAR_PROVIDER_ICON_MODE` | `svg`                            | `svg` for CodexBar SVG icons, `font` for mapped app-font glyphs with SVG fallback |
| `SHOWY_BAR_CODEXBAR_RESOURCES`      | `/Applications/CodexBar.app/...`       | Where to find provider SVGs                           |
| `SHOWY_BAR_SKETCHYBAR_COMPACT_PROVIDER_COUNT` | `5` | Provider-count breakpoint exposed by `showy-bar-state` for external layout managers |

Secondary and tertiary row colors auto-derive from the primary palette at
`0.55` by default. Override `SHOWY_BAR_PALETTE_SECONDARY_*`,
`SHOWY_BAR_PALETTE_TERTIARY_*`, or the `*_SCALE` knobs when you want custom
per-role colors.

## Verification

```sh
make doctor                         # bash 4+, jq, codexbar present
make test                           # smoke tests over JSON fixtures
make diagnose                       # printable bug-report state (= `bin/showy-bar --diagnose`)
bin/showy-bar-fetch | jq length     # 1+ if CodexBar has providers enabled
bin/showy-bar-state                 # JSON state for layout managers
bin/showy-bar --list                # available palette themes
bin/showy-bar --preview default     # deterministic ANSI theme preview
bin/showy-bar-zellij-bar            # ANSI strip
bin/showy-bar-tmux-bar              # tmux markup
```

Cache lives at `${XDG_CACHE_HOME:-~/.cache}/showy-bar/usage.json`.
`make clean` clears it.

## Theme gallery

Each preview uses the same two-provider fixture with `3:29` and `23m`
countdowns, good/warn/bad remaining-usage colors, and different pacing-marker
positions visible. SketchyBar previews are static renderings of the same
icon/row layout. Terminal previews show the deterministic
`showy-bar --preview` output.

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

## How it stays cheap

- One `codexbar` invocation per `SHOWY_BAR_REFRESH_SECONDS` regardless of how
  many bars are running.
- SketchyBar compares the desired provider set to its last declared state once
  per tick; add/remove and bracket rebuild only happen when that set changes.
- SVG fallback icon PNGs are generated once per provider per cache directory.
- Native SketchyBar rows and font icons avoid steady-state image generation.
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
