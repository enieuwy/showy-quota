---
name: deployed-config-is-not-repo-source
description: "Never write to chezmoi or a deployed config path — deployment lives outside this repo, and deployed files may be symlinks back into it"
condition: ['.*']
scope:
  - "tool:edit(**/chezmoi/**)"
  - "tool:write(**/chezmoi/**)"
  - "tool:edit(**/.config/sketchybar/**)"
  - "tool:write(**/.config/sketchybar/**)"
  - "tool:edit(**/.config/zellij/**)"
  - "tool:write(**/.config/zellij/**)"
  - "tool:edit(**/.local/bin/**)"
  - "tool:write(**/.local/bin/**)"
---

**This path is outside the repo.** Dotfile deployment and configuration live in `/Users/ellis/.local/share/chezmoi`; do not edit chezmoi or deployed config unless explicitly asked.

The reason it bites rather than merely being untidy: **deployed files under `~/.local/bin`, `~/.config/sketchybar`, and `~/.config/zellij` may be symlinks back into this repo** (`make install` symlinks scripts into `~/.local/bin` and the SketchyBar pieces into `~/.config/sketchybar` — `Makefile:3-4,13-14`). The same path can be either:

- **a symlink into the repo** — writing here silently edits tracked source through a path git never shows you; or
- **a real deployed copy** — writing here changes live behaviour that no commit records, and the next `make install` reverts it without warning.

Verify first, then edit the repo instead:

```bash
ls -l ~/.local/bin/showy-quota* ~/.config/sketchybar/plugins/showy_quota.sh
readlink -f ~/.config/zellij/plugins/showy-quota-zellij*.wasm
```

Symlink → the repo source is already live, no deploy needed. Real file → change the repo source and **report** the deploy command rather than running it.

If the user explicitly asked for a chezmoi or deployed-config edit, say which file and why the repo source cannot carry the change, then continue.
