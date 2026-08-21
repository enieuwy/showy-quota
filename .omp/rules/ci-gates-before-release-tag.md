---
name: ci-gates-before-release-tag
description: "Run make ci-gates before tagging a release — it is the only local command that runs every CI gate"
condition:
  - 'git tag\s+(-[asmfu]|[^-\s])'
  - 'git push[^\n]*--tags'
  - 'gh release create'
scope: ["tool:bash"]
interruptMode: never
---

**`make ci-gates` runs every CI gate locally — lint, test, fmt, clippy, `cargo test`, audit, plugin build, and the WASM export check (`Makefile:382`) — and it is documented as the thing to run before tagging a release.**

CI itself runs `make lint`, `make test`, `cargo test --workspace`, `make plugin`, and the WASM export check **on both Ubuntu and macOS**, so a tag pushed on the strength of `make test` alone can still go red — the shell suite is one of five gates, and half of them are Rust/WASM.

```bash
make ci-gates
```

Two gates that specifically do not run any other way: the **WASM export check** (a plugin that builds but exports the wrong symbols still passes `cargo test`) and **`cargo clippy`/`fmt`** on the workspace.

If `make ci-gates` has already passed on this exact tree, say so and continue.
