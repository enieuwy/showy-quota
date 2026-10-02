#!/usr/bin/env python3
"""Consumer-level cold-path checks; no native build, provider login, or live session."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
CLI = ROOT / "bin/showy-quota"


class ManagementTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix=".management-", dir=ROOT)
        self.directory = Path(self.temp.name)
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("SHOWY_QUOTA_")}
        self.env.update(HOME=str(self.directory), XDG_CONFIG_HOME=str(self.directory / "config"),
                        XDG_CACHE_HOME=str(self.directory / "cache"), TERM="xterm-256color")
        self.config = self.directory / "config/showy-quota/config.env"

    def tearDown(self):
        self.temp.cleanup()

    def command(self, *args, success=True):
        result = subprocess.run([str(CLI), *args], env=self.env, text=True, capture_output=True, cwd=ROOT)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def test_config_preserves_unrelated_bytes_and_roundtrips_quotes(self):
        self.config.parent.mkdir(parents=True)
        self.config.write_bytes(b"# keep spacing  \nUNRELATED='a b'\n# final comment")
        self.command("config", "set", "REFRESH_SECONDS", "60")
        self.assertEqual(self.command("config", "get", "REFRESH_SECONDS").stdout, "60\n")
        self.assertTrue(self.config.read_bytes().startswith(b"# keep spacing  \nUNRELATED='a b'\n# final comment\n"))
        value = "a 'quoted' value\n$(touch must-not-exist) `false` \\ end"
        self.command("config", "set", "SKETCHYBAR_CLICK", value)
        self.assertEqual(self.command("config", "get", "SKETCHYBAR_CLICK").stdout, value + "\n")
        self.assertFalse((ROOT / "must-not-exist").exists())
        self.command("config", "set", "SKETCHYBAR_CLICK", "replacement")
        self.assertEqual(self.command("config", "get", "SKETCHYBAR_CLICK").stdout, "replacement\n")
        before = self.config.read_bytes()
        self.command("config", "set", "NOT_A_SETTING", "x", success=False)
        self.command("config", "set", "REFRESH_SECONDS", "oops", success=False)
        self.assertEqual(self.config.read_bytes(), before)
        self.command("config", "unset", "REFRESH_SECONDS")
        self.assertNotIn(b"SHOWY_QUOTA_REFRESH_SECONDS=", self.config.read_bytes())
        self.assertEqual(self.command("config", "get", "REFRESH_SECONDS").stdout, "120\n")

    def test_config_preserves_export_for_child_processes(self):
        self.config.parent.mkdir(parents=True)
        self.config.write_text("export SHOWY_QUOTA_REFRESH_SECONDS=90\n")
        self.command("config", "set", "REFRESH_SECONDS", "60")
        result = subprocess.run(
            ["bash", "-c", '. "$1"; exec env', "probe", str(self.config)],
            env=self.env, text=True, capture_output=True, check=True)
        self.assertIn("SHOWY_QUOTA_REFRESH_SECONDS=60", result.stdout.splitlines())

    def test_theme_import_and_untrusted_manifest(self):
        palette = {f"base{i:02X}": f"{i:02x}2233" for i in range(16)}
        source = self.directory / "palette.json"
        source.write_text(json.dumps(palette))
        self.command("theme", "import-base16", str(source), "fixture")
        value = json.loads(self.command("theme", "show", "fixture").stdout)
        self.assertEqual(value["SHOWY_QUOTA_PALETTE_PRIMARY_GOOD"], "0b2233")
        self.assertEqual(value["SHOWY_QUOTA_PALETTE_PRIMARY_BAD"], "082233")
        self.command("--set", "fixture")
        self.assertEqual(self.command("config", "get", "PALETTE_PRIMARY_GOOD").stdout, "0b2233\n")
        self.command("theme", "new", "fixture", success=False)
        bad = self.config.parent / "themes/bad.json"
        bad.write_text(json.dumps({"SHOWY_QUOTA_CODEXBAR_BIN": "touch must-not-exist"}))
        self.command("--set", "bad", success=False)
        self.assertFalse((ROOT / "must-not-exist").exists())
        self.assertEqual(self.command("--current").stdout, "fixture\n")

    def test_config_policy_bounds_timezone_and_empty_theme_glyph(self):
        policy = "claude=good:70,warn:30,codex=good:60"
        self.command("config", "set", "PROVIDER_THRESHOLDS", policy)
        self.assertEqual(self.command("config", "get", "PROVIDER_THRESHOLDS").stdout, policy + "\n")
        self.command("config", "set", "PROVIDER_THRESHOLDS", "claude=good:101", success=False)
        self.assertEqual(self.command("config", "get", "PROVIDER_THRESHOLDS").stdout, policy + "\n")
        for timezone in ("UTC", "Z", "+23:59"):
            self.command("config", "set", "RESET_DESCRIPTION_TIMEZONE_OFFSET", timezone)
            self.assertEqual(self.command("config", "get", "RESET_DESCRIPTION_TIMEZONE_OFFSET").stdout, timezone + "\n")
        self.command("config", "set", "RESET_DESCRIPTION_TIMEZONE_OFFSET", "+24:00", success=False)
        theme = self.config.parent / "themes/caps.json"
        theme.parent.mkdir(parents=True)
        theme.write_text(json.dumps({"SHOWY_QUOTA_CAP_LEFT": "[", "SHOWY_QUOTA_CAP_RIGHT": "]"}))
        self.command("--set", "caps")
        self.command("config", "set", "CAP_LEFT", "")
        self.assertEqual(self.command("config", "get", "CAP_LEFT").stdout, "\n")
        self.assertEqual(self.command("config", "get", "CAP_RIGHT").stdout, "]\n")

    def test_iterm_import_maps_rgb_components(self):
        import plistlib
        palette = {key: {"Red Component": 1.0, "Green Component": 0.5, "Blue Component": 0.0}
                   for key in ("Ansi 0 Color", "Ansi 1 Color", "Ansi 2 Color", "Ansi 3 Color",
                               "Ansi 5 Color", "Ansi 7 Color", "Ansi 8 Color", "Background Color", "Foreground Color")}
        source = self.directory / "fixture.itermcolors"
        source.write_bytes(plistlib.dumps(palette))
        self.command("theme", "import-iterm", str(source), "iterm-fixture")
        value = json.loads(self.command("theme", "show", "iterm-fixture").stdout)
        self.assertEqual(value["SHOWY_QUOTA_PALETTE_PRIMARY_GOOD"], "ff8000")

    def test_terminal_doctor_does_not_claim_font_detection(self):
        self.env.update(TERM_PROGRAM="WezTerm", ALACRITTY_WINDOW_ID="fixture")
        report = json.loads(self.command("--check-terminal", "--json").stdout)
        self.assertEqual(report["terminal"], "Alacritty")
        self.assertEqual(report["glyphSupport"], "unverified")
        self.assertEqual(report["recommendation"], "portable")
        self.assertEqual(report["samples"]["caps"], "")

    def test_tmux_move_update_off_restore(self):
        socket_dir = ROOT / ".t"
        socket_dir.mkdir(exist_ok=True)
        self.env["TMUX_TMPDIR"] = str(socket_dir)
        self.env.pop("TMUX", None)
        socket = "q" + str(os.getpid())

        def tmux(*args):
            result = subprocess.run(["tmux", "-L", socket, *args], env=self.env,
                                    text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            return result.stdout.rstrip("\n")

        tmux("-f", "/dev/null", "new-session", "-d", "-s", "fixture", "sleep 120")
        try:
            tmux("set-option", "-g", "status-right", "RIGHT")
            tmux("set-option", "-g", "status-left", "LEFT")
            tmux("set-option", "-g", "status-right-length", "49")
            tmux("bind-key", "-T", "prefix", "F12", "display-message", "ORIGINAL")
            original_binding = tmux("list-keys", "-T", "prefix", "F12")
            tmux("set-option", "-g", "@showy-quota-popup-key", "F12")
            for invalid_option, invalid_value in (
                    ("@showy-quota-popup-height", "invalid"),
                    ("@showy-quota-popup-width", "101%"),
                    ("@showy-quota-popup-key", "DefinitelyNotAKey")):
                original_options = tmux("show-options", "-g")
                tmux("set-option", "-g", invalid_option, invalid_value)
                before = tmux("show-options", "-g")
                self.command("tmux", "sync", "--socket", socket, success=False)
                self.assertEqual(tmux("show-options", "-g"), before)
                self.assertEqual(tmux("list-keys", "-T", "prefix", "F12"), original_binding)
                if invalid_option == "@showy-quota-popup-key":
                    tmux("set-option", "-g", invalid_option, "F12")
                else:
                    tmux("set-option", "-gu", invalid_option)
                self.assertEqual(tmux("show-options", "-g"), original_options)
            dry = self.command("tmux", "sync", "--socket", socket, "--dry-run", "--json")
            self.assertFalse(json.loads(dry.stdout)["applied"])
            self.assertEqual(tmux("show-options", "-gqv", "status-right"), "RIGHT")
            self.command("tmux", "sync", "--socket", socket)
            first = tmux("show-options", "-gqv", "status-right")
            self.command("tmux", "sync", "--socket", socket)
            self.assertEqual(tmux("show-options", "-gqv", "status-right"), first)
            self.assertEqual(first.count("showy-quota-tmux-bar"), 1)
            tmux("set-option", "-g", "@showy-quota-position", "left")
            self.command("tmux", "sync", "--socket", socket)
            self.assertEqual(tmux("show-options", "-gqv", "status-right"), "RIGHT")
            self.assertEqual(tmux("show-options", "-gqv", "status-right-length"), "49")
            self.assertEqual(tmux("show-options", "-gqv", "status-left").count("showy-quota-tmux-bar"), 1)
            report = json.loads(self.command("--tmux-doctor", "--socket", socket, "--json").stdout)
            self.assertEqual(report["position"], "left")
            self.assertEqual(report["popupKey"], "F12")
            tmux("set-option", "-g", "@showy-quota-position", "off")
            self.command("tmux", "sync", "--socket", socket)
            self.assertEqual(tmux("show-options", "-gqv", "status-left"), "LEFT")
            self.assertEqual(tmux("list-keys", "-T", "prefix", "F12"), original_binding)
        finally:
            tmux("kill-server")
            # This test owns only its socket; tmux removes it on shutdown.
            uid_dir = socket_dir / ("tmux-" + str(os.getuid()))
            try:
                uid_dir.rmdir()
                socket_dir.rmdir()
            except OSError:
                pass


if __name__ == "__main__":
    unittest.main(verbosity=2)
