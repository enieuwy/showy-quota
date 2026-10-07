#!/usr/bin/env python3
"""Inspect and reconcile only the tmux state that showy-quota owns."""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
STATE_KEY = "@showy-quota-managed-state"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("sync", "doctor"))
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--socket", help="isolated tmux server socket name (-L)")
    parser.add_argument("--tpm", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    prefix = ["tmux"] + (["-L", args.socket] if args.socket else [])

    def run(*values, check=True):
        result = subprocess.run(prefix + list(values), text=True, capture_output=True)
        if check and result.returncode:
            raise ValueError(result.stderr.strip() or "tmux command failed")
        return result.stdout.rstrip("\n")

    def read_binding(key):
        # Some tmux versions return no rows when list-keys gets a key argument.
        # Read the table so ownership checks compare the actual binding.
        for line in run("list-keys", "-T", "prefix", check=False).splitlines():
            tokens = shlex.split(line)
            if "-T" in tokens and tokens[tokens.index("-T") + 2] == key:
                return line
        return ""

    options = {}
    for line in run("show-options", "-g").splitlines():
        key, _, _ = line.partition(" ")
        options[key] = run("show-options", "-gqv", key)

    def option(name, default):
        return options.get(name, default)

    previous = json.loads(option(STATE_KEY, "{}") or "{}")
    if not isinstance(previous, dict):
        raise ValueError("invalid managed state; refusing to change tmux")
    renderer = os.path.expanduser(option("@showy-quota-bin", str(ROOT / "bin/showy-quota-tmux-bar")))
    side = option("@showy-quota-position", "right")
    if side in ("off", "none", "disabled"):
        side = "off"
    if side not in ("left", "right", "off"):
        raise ValueError("@showy-quota-position must be left, right, or off")
    length = option("@showy-quota-status-length", "300")
    if not length.isdecimal() or not 1 <= int(length) <= 10000:
        raise ValueError("@showy-quota-status-length must be an integer from 1 to 10000")
    separator = option("@showy-quota-separator", " ")
    if any(ord(c) < 32 for c in renderer + separator):
        raise ValueError("renderer and separator must not contain control characters")
    executable = os.path.isfile(renderer) and os.access(renderer, os.X_OK)
    renderer_error = None
    if re.search(r"""[$`"'\\;&|<>()\s]""", renderer):
        renderer_error = "renderer path contains shell metacharacters: " + renderer
    elif not executable:
        renderer_error = "renderer is not executable: " + renderer
    if side != "off" and renderer_error and args.action == "sync" and not args.dry_run:
        if args.tpm:
            run("display-message", "showy-quota: " + renderer_error, check=False)
            print("showy-quota: " + renderer_error, file=sys.stderr)
            return
        raise ValueError(renderer_error)
    status = {s: option("status-" + s, "") for s in ("left", "right")}
    desired = dict(status)
    commands = []
    old_side = previous.get("side")
    segment = previous.get("segment", "")
    if old_side in desired and segment:
        desired[old_side] = desired[old_side].replace(segment, "")
    new_state = {}
    if side != "off" and executable:
        # Shell quoting happens before tmux escaping; both interpreters see data.
        command = shlex.quote(renderer).replace("#", "##")
        new_segment = separator.replace("#", "##") + "#(" + command + ")"
        desired[side] += new_segment
        new_state.update(side=side, segment=new_segment)
    for s in ("left", "right"):
        if desired[s] != status[s]:
            commands.append(["set-option", "-gq", "status-" + s, desired[s]])
    old_length = previous.get("length")
    if old_side in ("left", "right") and old_length:
        key = "status-" + old_side + "-length"
        if option(key, "") == old_length.get("managed"):
            if side != old_side or not executable:
                commands.append(["set-option", "-gq", key, old_length["original"]])
    if side != "off" and executable:
        key = "status-" + side + "-length"
        current = option(key, "0")
        original = old_length["original"] if old_side == side and old_length and current == old_length["managed"] else current
        managed = str(max(int(original), int(length)))
        new_state["length"] = {"original": original, "managed": managed}
        if current != managed:
            commands.append(["set-option", "-gq", key, managed])

    popup_key = option("@showy-quota-popup-key", "")
    if side == "off" or popup_key in ("off", "none", "disabled"):
        popup_key = ""
    if popup_key and not re.fullmatch(r"[A-Za-z0-9+!@#$%^&*()_={}\[\]:;,.?/~<>|-]+", popup_key):
        raise ValueError("unsupported popup key")
    old_popup = previous.get("popup", {})
    old_key = old_popup.get("key")
    binding = read_binding(old_key) if old_key else ""
    if old_key and binding == old_popup.get("managed"):
        if old_key != popup_key:
            commands.append(shlex.split(old_popup["original"]) if old_popup.get("original") else ["unbind-key", "-T", "prefix", old_key])
    popup = None
    if popup_key:
        current_binding = read_binding(popup_key)
        original_binding = old_popup.get("original", "") if old_key == popup_key and current_binding == old_popup.get("managed") else current_binding
        interval = option("@showy-quota-popup-interval", "30")
        if not interval.isdecimal() or not 1 <= int(interval) <= 86400:
            raise ValueError("@showy-quota-popup-interval must be from 1 to 86400")
        dimensions = {}
        for dimension, default in (("height", "36"), ("width", "92")):
            value = option("@showy-quota-popup-" + dimension, default)
            if not re.fullmatch(r"[1-9][0-9]{0,4}%?", value):
                raise ValueError("popup " + dimension + " must be a positive size or percentage")
            if value.endswith("%") and int(value[:-1]) > 100:
                raise ValueError("popup " + dimension + " percentage must not exceed 100")
            dimensions[dimension] = value
        popup_command = (
            'config="${XDG_CONFIG_HOME:-$HOME/.config}/showy-quota/config.env"; '
            '[ -r "$config" ] && . "$config"; while :; do clear; '
            '"${SHOWY_QUOTA_CODEXBAR_BIN:-codexbar}" usage; sleep ' + interval + '; done'
        )
        bind = ["bind-key", "-T", "prefix", popup_key, "display-popup", "-E",
                "-h", dimensions["height"],
                "-w", dimensions["width"],
                "-T", option("@showy-quota-popup-title", "CodexBar usage").replace("#", "##"), popup_command]
        commands.append(bind)
        popup = {"key": popup_key, "original": original_binding}
        new_state["popup"] = popup
    duplicates = sum(text.count("showy-quota-tmux-bar") for text in status.values())
    report = {"renderer": renderer, "executable": executable, "position": side,
              "statusLength": int(length), "separator": separator, "popupKey": popup_key,
              "managed": previous, "status": status, "desiredStatus": desired,
              "duplicateSegments": max(0, duplicates - 1), "commands": [prefix + c for c in commands],
              "repair": shlex.join([str(ROOT / "bin/showy-quota"), "tmux", "sync"] + (["--socket", args.socket] if args.socket else [])),
              "applied": args.action == "sync" and not args.dry_run}
    if report["applied"]:
        undo = []
        try:
            for command in commands:
                if command[0] == "set-option":
                    key = command[2]
                    old = run("show-options", "-gqv", key)
                    run(*command)
                    undo.append(("option", key, old, command[3]))
                else:
                    key = command[3]
                    old = read_binding(key)
                    run(*command)
                    current = read_binding(key)
                    undo.append(("binding", key, old, current))
            if popup:
                popup["managed"] = read_binding(popup_key)
            run("set-option", "-gq", STATE_KEY, json.dumps(new_state, separators=(",", ":")))
        except (ValueError, OSError) as error:
            recovery_errors = []
            for kind, key, original, applied in reversed(undo):
                try:
                    if kind == "option":
                        if run("show-options", "-gqv", key) == applied:
                            run("set-option", "-gq", key, original)
                    elif read_binding(key) == applied:
                        restore = shlex.split(original) if original else ["unbind-key", "-T", "prefix", key]
                        run(*restore)
                except (ValueError, OSError) as recovery_error:
                    recovery_errors.append(str(recovery_error))
            if recovery_errors:
                raise ValueError(f"{error}; rollback incomplete: {'; '.join(recovery_errors)}") from error
            raise
        run("refresh-client", "-S", check=False)
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"renderer: {renderer} (executable={executable})")
        print(f"position: {side}; length: {length}; popup: {popup_key or 'off'}")
        print(f"duplicate segments: {report['duplicateSegments']}")
        for command in report["commands"]:
            print(shlex.join(command))
        print("repair: " + report["repair"])


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError) as error:
        print(f"showy-quota tmux: {error}", file=sys.stderr)
        sys.exit(1)
