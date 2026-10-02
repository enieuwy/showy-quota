#!/usr/bin/env python3
"""Cold-path configuration and theme tools. No provider or network access."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
CONFIG = Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "showy-quota"
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*\Z")


def atomic_write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def shell_quote(value):
    if not any(ord(c) < 32 or ord(c) == 127 for c in value):
        return shlex.quote(value)
    encoded = "".join(
        "\\\\" if c == "\\" else "\\'" if c == "'" else
        f"\\x{ord(c):02x}" if ord(c) < 32 or ord(c) == 127 else c
        for c in value
    )
    return "$'" + encoded + "'"


def config_write(action, key, value=""):
    path = CONFIG / "config.env"
    original = path.read_bytes() if path.exists() else b""
    pattern = re.compile(rb"^([ \t]*(?:export[ \t]+)?)" + key.encode() + rb"=")
    output, inserted = [], False
    for line in original.splitlines(keepends=True):
        match = pattern.match(line)
        if match:
            if action == "set" and not inserted:
                output.append(match[1] + (key + "=" + shell_quote(value) + "\n").encode())
                inserted = True
        else:
            output.append(line)
    if action == "set" and not inserted:
        if output and not output[-1].endswith(b"\n"):
            output.append(b"\n")
        output.append((key + "=" + shell_quote(value) + "\n").encode())
    result = b"".join(output)
    if result != original:
        syntax = subprocess.run(["bash", "-n"], input=result, capture_output=True)
        if syntax.returncode:
            raise ValueError("configuration rewrite is not valid shell syntax; file unchanged")
        atomic_write(path, result)


def catalog():
    return json.loads((ROOT / "share/themes/catalog.json").read_text())


def validate_theme(value):
    result = subprocess.run(["jq", "-e", "-L", str(ROOT / "share/themes"),
                             'include "validate"; valid_theme'],
                            input=json.dumps(value), text=True, capture_output=True)
    if result.returncode:
        raise ValueError("theme contains an unknown key or an invalid palette/glyph value")
    return value


def theme_data(name):
    if not NAME.fullmatch(name):
        raise ValueError("invalid theme name")
    path = CONFIG / "themes" / (name + ".json")
    value = json.loads(path.read_text()) if path.exists() else catalog().get(name)
    return validate_theme(value)


def theme_command(args):
    action = args[0] if args else "list"
    if action == "list":
        names = set(catalog())
        directory = CONFIG / "themes"
        if directory.exists():
            names.update(p.stem for p in directory.glob("*.json"))
        for name in sorted(names):
            theme_data(name)
            print(name)
        return
    if action in ("show", "validate") and len(args) == 2:
        value = theme_data(args[1])
        if action == "show":
            print(json.dumps(value, indent=2))
        return
    if action not in ("new", "import-base16", "import-iterm"):
        raise ValueError("theme expects list, show, validate, new, import-base16, or import-iterm")
    expected = 2 if action == "new" else 3
    if len(args) != expected:
        raise ValueError("theme new NAME | theme import-base16|import-iterm FILE NAME")
    name = args[-1]
    if not NAME.fullmatch(name):
        raise ValueError("invalid theme name")
    path = CONFIG / "themes" / (name + ".json")
    if path.exists() or name in catalog():
        raise ValueError("theme already exists; choose a new name")
    value = dict(catalog()["default"])
    if action == "import-base16":
        text = Path(args[1]).read_text()
        try:
            scheme = json.loads(text)
            scheme = scheme.get("palette", scheme)
        except json.JSONDecodeError:
            scheme = {}
            for line in text.splitlines():
                match = re.match(r'^\s*(base[0-9a-fA-F]{2}):\s*[\"\']?#?([0-9a-fA-F]{6})[\"\']?\s*(?:#.*)?$', line)
                if match:
                    scheme[match[1]] = match[2]
        scheme = {key.lower(): str(color).lstrip("#") for key, color in scheme.items()}
        roles = {"PRIMARY_GOOD": "base0b", "PRIMARY_WARN": "base0a", "PRIMARY_BAD": "base08",
                 "PRIMARY_UNKNOWN": "base03", "BG": "base00", "SURFACE": "base01",
                 "TRACK": "base02", "ICON_TEXT": "base05", "COUNTDOWN": "base04",
                 "COUNTDOWN_WARN": "base08", "ELAPSED": "base0e"}
        for role, key in roles.items():
            if key not in scheme:
                raise ValueError("base16 palette is missing " + key)
            value["SHOWY_QUOTA_PALETTE_" + role] = scheme[key]
    elif action == "import-iterm":
        with Path(args[1]).open("rb") as source:
            scheme = plistlib.load(source)
        roles = {"PRIMARY_GOOD": "Ansi 2 Color", "PRIMARY_WARN": "Ansi 3 Color",
                 "PRIMARY_BAD": "Ansi 1 Color", "PRIMARY_UNKNOWN": "Ansi 8 Color",
                 "BG": "Background Color", "SURFACE": "Ansi 0 Color", "TRACK": "Ansi 8 Color",
                 "ICON_TEXT": "Foreground Color", "COUNTDOWN": "Ansi 7 Color",
                 "COUNTDOWN_WARN": "Ansi 1 Color", "ELAPSED": "Ansi 5 Color"}
        for role, key in roles.items():
            color = scheme[key]
            components = [color[channel + " Component"] for channel in ("Red", "Green", "Blue")]
            if not all(isinstance(c, (int, float)) and 0 <= c <= 1 for c in components):
                raise ValueError("iTerm color components must be in [0,1]")
            value["SHOWY_QUOTA_PALETTE_" + role] = "".join(f"{round(c * 255):02x}" for c in components)
    validate_theme(value)
    atomic_write(path, (json.dumps(value, indent=2) + "\n").encode())
    print(path)


def terminal_doctor(as_json):
    terminal = os.environ.get("TERM_PROGRAM", "unknown")
    if os.environ.get("ALACRITTY_WINDOW_ID"):
        terminal = "Alacritty"
    if os.environ.get("KITTY_WINDOW_ID"):
        terminal = "kitty"
    # Environment identifies a host, not its font. Only the operator can verify glyphs.
    recommendation = "portable"
    result = {"terminal": terminal, "term": os.environ.get("TERM", ""),
              "glyphSupport": "unverified", "recommendation": recommendation,
              "config": "SHOWY_QUOTA_TERMINAL_BAR_MODE=portable",
              "samples": {"ascii": "[####....] 50%", "caps": "", "halfBlocks": "▀▄",
                          "sextants": "🬀🬁🬂", "octants": "\U0001cd00\U0001cd01\U0001cd02"}}
    if as_json:
        print(json.dumps(result, ensure_ascii=False))
    else:
        print(f"Terminal: {terminal}; TERM={result['term']}")
        for key, value in result["samples"].items():
            print(f"{key}: {value}")
        print("Font coverage cannot be detected from terminal environment variables.")
        print("If any sample shows boxes, use SHOWY_QUOTA_TERMINAL_BAR_MODE=portable.")
        print("If half-blocks work, use antigravity=dual2 in SHOWY_QUOTA_PROVIDER_MODES.")
        print("Use mono4 only after you confirm the octant samples display correctly.")

def validate_config(key, value):
    rows = json.loads((ROOT / "share/config-manifest.json").read_text())["settings"]
    spec = next((row for row in rows if row["key"] == key), None)
    if spec is None:
        raise ValueError("unknown configuration key: " + key)
    if "\0" in value:
        raise ValueError("configuration values must not contain NUL")
    if not value and spec.get("nullable"):
        return
    kind = spec["type"]
    valid = True
    if "allowed" in spec:
        valid = value in spec["allowed"]
    elif kind == "integer":
        valid = bool(re.fullmatch(r"[0-9]+", value)) and spec.get("min", 0) <= int(value) <= spec.get("max", 2**63 - 1)
    elif kind == "boolean":
        valid = value.strip().lower() in ("1", "0", "true", "false", "yes", "no", "on", "off")
    elif kind == "color":
        valid = bool(re.fullmatch(r"[0-9a-fA-F]{6}", value))
    elif kind == "glyph":
        valid = len(value) <= 16 and not any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value)
    elif kind == "timezone":
        clean = value.strip()
        valid = not clean or clean.lower() == "utc" or clean == "Z" or bool(re.fullmatch(r"[+-](?:[01][0-9]|2[0-3]):[0-5][0-9]", clean))
    elif kind == "thresholds":
        scope = ""
        for token in re.split(r"[;,]", value):
            token = token.strip()
            if not token:
                continue
            if "=" in token:
                scope, token = (part.strip() for part in token.split("=", 1))
            if not re.fullmatch(r"[A-Za-z0-9_.-]+", scope):
                valid = False
                break
            match = re.fullmatch(r"(good|warn|time)\s*:\s*([0-9]+)", token)
            if not match or int(match[2]) > (2**63 - 1 if match[1] == "time" else 100):
                valid = False
                break
    elif kind == "provider_modes":
        valid = all(re.fullmatch(r"[A-Za-z0-9_.-]+=(?:auto|dual|dual2|mono3|mono4|sextant3|portable)", clause.strip()) for clause in value.split(",") if clause.strip())
    if not valid:
        raise ValueError(f"{key}: invalid {kind} value")



def main():
    args = sys.argv[1:]
    if not args:
        raise ValueError("missing management command")
    if args[0] == "config-write":
        config_write(*args[1:])
    elif args[0] == "config-validate":
        validate_config(*args[1:])
    elif args[0] == "theme":
        theme_command(args[1:])
    elif args[0] == "terminal-doctor":
        if args[1:] not in ([], ["--json"]):
            raise ValueError("terminal doctor accepts only --json")
        terminal_doctor("--json" in args)
    else:
        raise ValueError("unknown management command")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"showy-quota: {error}", file=sys.stderr)
        sys.exit(2)
