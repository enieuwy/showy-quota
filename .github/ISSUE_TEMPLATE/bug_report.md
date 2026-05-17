---
name: Bug report
about: Something on the bar is wrong, missing, or broken.
title: ""
labels: bug
---

<!--
Thanks for the report. Please paste real output below — the diagnose command
already redacts secrets (it only echoes path locations and tool versions).
-->

## What did you expect?

<!-- e.g. "Claude row should appear after I run make install-sketchybar." -->

## What happened instead?

## Which bar are you wiring?

- [ ] SketchyBar (macOS)
- [ ] tmux
- [ ] Zellij

## Diagnose output

<details>
<summary><code>showy-bar --diagnose</code></summary>

```
$ bin/showy-bar --diagnose
<paste the full output here>
```

</details>

## CodexBar sanity

```
$ codexbar usage --format json | jq length
```

Replace `<n>` with the number you saw. If it's 0, the issue is most likely
upstream in CodexBar (provider not enabled / not signed in).

## Environment

- OS:
- `bash --version | head -n1`:
- showy-bar commit:
- CodexBar version (`codexbar --version`):

## Anything else?
