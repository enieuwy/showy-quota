# codexbar-bars

Always-on AI coding-quota strips for **SketchyBar**, **Zellij**, and **tmux**,
driven entirely by the [CodexBar](https://github.com/steipete/CodexBar) CLI.

CodexBar handles every provider's auth, cookies, OAuth, parsing, and caching.
This repo's only job is to render its JSON in three places:

```
codexbar usage --format json
       │
       ▼
bin/cb-bars-fetch     ←  shared cache + flock + last-known-good
       │  ~/.cache/codexbar-bars/usage.json
       ├──► sketchybar/plugins/cb_bars.sh    (per-provider PNG icon + bar)
       ├──► bin/cb-bars-zellij-bar           (ANSI strip for zjstatus pipe)
       └──► bin/cb-bars-tmux-bar             (tmux #[…] markup for status-right)
```

No sidecar, no daemon, no provider auth code, no extra config beyond a
single optional env file.

## Requirements

- **macOS** (SketchyBar is mac-only; the Zellij/tmux bars work on Linux too).
- [`codexbar`](https://github.com/steipete/CodexBar) on the PATH and
  configured. Install with `brew install --cask steipete/tap/codexbar` and
  enable providers in **CodexBar → Preferences → Providers**.
- `bash` 4+, `jq`, ImageMagick 7+ (`magick`), and a `date` that understands
  either `-j -f` (BSD/macOS) or `-d` (GNU coreutils).
- Optional: `flock` for inter-process locking; falls back to a `mkdir`
  lease when missing.

The SketchyBar plugin sources provider icons from
`/Applications/CodexBar.app/Contents/Resources/ProviderIcon-<id>.svg`. No
icons are bundled in this repo.

## Install

```sh
git clone https://github.com/<you>/codexbar-bars ~/dev/codexbar-bars
cd ~/dev/codexbar-bars
make install                   # symlinks bin/* into ~/.local/bin and
                               # SketchyBar pieces into ~/.config/sketchybar
```

`make install` refuses to clobber existing non-symlink files, so a
pre-existing `~/.config/sketchybar/items/cb_bars.sh` will fail loudly
rather than be overwritten.

To uninstall:

```sh
make uninstall
```

### SketchyBar wiring

Add to `~/.config/sketchybar/sketchybarrc`, after `ITEM_DIR` and
`PLUGIN_DIR` are defined:

```sh
source "$ITEM_DIR/cb_bars.sh"
```

Then reload SketchyBar (`sketchybar --reload` or quit + relaunch). One
icon + bar + label triple appears per provider currently fetching usage
data; clicks bring CodexBar.app forward.

### Zellij wiring

Two pieces:

1. **Pipe loop** — paste `zellij/layout-pane.kdl.fragment` into your
   default layout (the `pane size=1` widget plus the hidden 1%×1%
   command pane that runs `cb-bars-zellij-pipe`).
2. **Detail keybind** — paste `zellij/detail-pane.kdl.fragment` into your
   keybinds block. Default is `Alt /`.

Reload Zellij to pick up the new layout.

### tmux wiring

```sh
# Use the absolute path — tmux's PATH at server start typically lacks ~/.local/bin.
CB_BIN="$HOME/.local/bin"
printf 'set -ag status-right " #(%s/cb-bars-tmux-bar)"\n' "$CB_BIN" >> ~/.tmux.conf
printf 'bind-key "/" display-popup -E -h 36 -w 92 -T "CodexBar usage" %s\n' \
    "'while :; do clear; codexbar usage; sleep 30; done'" >> ~/.tmux.conf
tmux source ~/.tmux.conf
```

No `watch(1)` dependency — the popup uses a tiny shell loop so this works on a
stock macOS install.

## Configuration

Every script reads optional overrides from
`~/.config/codexbar-bars/config.env` (see
[`share/config.env.example`](share/config.env.example) for the full list
of variables). All values have working defaults; the file is optional.

Useful knobs:

| Variable                          | Default                                | Effect                                                |
|-----------------------------------|----------------------------------------|-------------------------------------------------------|
| `CB_BARS_REFRESH_SECONDS`         | `120`                                  | Upper bound on how often `codexbar` itself is invoked |
| `CB_BARS_PROVIDERS`               | empty (use whatever CodexBar enables)  | Comma-list filter, e.g. `claude,codex`                |
| `CB_BARS_TIME_WARN_MINUTES`       | `30`                                   | Threshold for red countdown labels                    |
| `CB_BARS_PALETTE_GOOD/WARN/BAD`   | Catppuccin Macchiato                   | 6-char hex (no `#`)                                   |
| `CB_BARS_SKETCHYBAR_CLICK`        | `open -b com.steipete.codexbar`        | Click action on a SketchyBar item                     |
| `CB_BARS_CODEXBAR_RESOURCES`      | `/Applications/CodexBar.app/...`       | Where to find provider SVGs                           |

## Verification

```sh
make test                         # 18 smoke tests over JSON fixtures
bin/cb-bars-fetch | jq length     # 1+ if CodexBar has providers enabled
bin/cb-bars-zellij-bar            # ANSI strip
bin/cb-bars-tmux-bar              # tmux markup
```

Cache lives at `${XDG_CACHE_HOME:-~/.cache}/codexbar-bars/usage.json`.
`make clean` clears it.

## How it stays cheap

- One `codexbar` invocation per `CB_BARS_REFRESH_SECONDS` regardless of how
  many bars are running.
- SketchyBar's plugin only writes a PNG when its bytes change (atomic
  `cmp`-then-`mv`).
- Provider icon PNGs are generated once per provider per cache directory.
- Bars never blank on transient `codexbar` failure: the fetcher serves
  the last-known-good cache and exits 0.

## Limitations

- `codexbar` runs from a GUI macOS app bundle; cookie-based providers
  need Full Disk Access in System Settings → Privacy & Security to
  decrypt browser cookies.
- The strip omits CodexBar's `tertiary` window for tmux/Zellij (only
  primary + an optional `w` worse-than-primary hint for secondary).
  SketchyBar shows up to three stacked bars when the provider exposes a
  tertiary window.
- **Stale-cache dimming is terminal-only.** When the cache is older than
  `2 × CB_BARS_REFRESH_SECONDS`, the Zellij and tmux strips dim every
  provider chunk. SketchyBar continues to render at full strength —
  CodexBar's own menu icon will reflect upstream incidents.
- **New providers require a SketchyBar reload.** The item set is built
  when sketchybarrc sources `cb_bars.sh`. If you enable a new provider in
  CodexBar later, run `sketchybar --reload` (or quit + relaunch) to make
  the new icon/bar/label triple appear. Zellij and tmux pick up new
  providers automatically on the next refresh.
- No Linux-side provider for browser-cookie providers — same constraint
  as CodexBar itself.

## Layout

```
bin/             cb-bars-fetch, cb-bars-{zellij,tmux}-bar, cb-bars-zellij-pipe
lib/             common.sh, strip.sh
sketchybar/      items/cb_bars.sh, plugins/cb_bars.sh
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
