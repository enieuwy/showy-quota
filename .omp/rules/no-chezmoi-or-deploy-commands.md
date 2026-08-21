---
name: no-chezmoi-or-deploy-commands
description: "Deploying is a user decision: chezmoi apply, make install-*, and hand-copying the WASM plugin all change live state outside this repo"
condition:
  - 'chezmoi (apply|add|re-add|edit|update|forget|destroy)'
  - 'make [^\n]*install(-[a-z]+)?\b'
  - '(cp|mv|ln|tee|rm)[^\n]*\.config/(sketchybar|zellij)'
  - '(cp|mv|ln|tee|rm)[^\n]*\.local/bin'
scope: ["tool:bash"]
interruptMode: always
---

**Rebuild/deploy only when explicitly deploying.** Otherwise report the source changes and the deploy command the user should run.

These commands all reach outside the repo into live user state:

- `make install`, `install-bin`, `install-sketchybar`, `install-plugin`, `install-all` (`Makefile:44,52,178,215,235`) symlink or copy into `~/.local/bin`, `~/.config/sketchybar`, `~/.config/zellij/plugins`.
- `chezmoi apply` rewrites deployed dotfiles from a source of truth that lives **outside this repo** (`/Users/ellis/.local/share/chezmoi`).
- Hand-copying the built `.wasm` into `~/.config/zellij/plugins/` deploys a build no `make` target recorded.

Two specific hazards:

1. **A deploy can change behaviour in a running SketchyBar or Zellij session** the user is looking at, and an active Zellij session can retain stale layout/plugin configuration afterwards — the fix is a restart/reload/new tab, which is also the user's call.
2. **Some deployed paths are already symlinks into this repo**, so a "deploy" may be a no-op that merely looks like progress — or may clobber a real file. `make install` refuses to clobber files or retarget existing symlinks (`Makefile:4`), which is protection you lose the moment you hand-roll it with `cp`/`ln`.

Building is fine and needs no permission: `make plugin`, `cargo build`, `make test`, `make doctor`, `make diagnose`. Run `make plugin` before claiming the plugin builds.

If the user asked for a deploy, name the exact target and continue.
