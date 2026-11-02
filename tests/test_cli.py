import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


REPO = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("luabuddy_cli", REPO / "luabuddy.py")
CLI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CLI)


class CliCheck(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="luabuddy-cli-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        shutil.copy2(REPO / "luabuddy.py", self.root / "luabuddy.py")
        shutil.copytree(REPO / "profiles", self.root / "profiles")
        (self.root / "lib").mkdir()
        (self.root / "dist").mkdir()
        (self.root / "lib/ZombieBuddy.jar").write_bytes(b"framework fixture")
        (self.root / "dist/LuaBuddy.jar").write_bytes(b"adapter fixture")
        self.game = self.root / "Steam library/steamapps/common/ProjectZomboid/projectzomboid"
        self.game.mkdir(parents=True)
        (self.game / "projectzomboid.jar").touch()
        self.launcher = self.game / "ProjectZomboid64.json"
        self.original = {"mainClass": "zombie/gameStates/MainScreenState", "classpath": [".", "projectzomboid.jar"],
                         "vmArgs": ["-Xmx3g", "-Dunrelated=preserve me", "-javaagent:other.jar"], "custom": {"value": 7}}
        self.save(self.original)

    def save(self, value):
        self.launcher.write_text(json.dumps(value, indent=4) + "\n", encoding="utf-8")

    def run_cli(self, *args, success=True, environment=None, preexec_fn=None):
        env = os.environ.copy()
        env.pop("PZ_GAME_DIR", None)
        if environment:
            env.update(environment)
        selection = [] if args[0] == "diagnostics" else ["--game-dir", str(self.game.parent)]
        result = subprocess.run([sys.executable, str(self.root / "luabuddy.py"), args[0],
                                 *selection, *args[1:]], text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, env=env, timeout=20, preexec_fn=preexec_fn)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def current(self):
        return json.loads(self.launcher.read_text(encoding="utf-8"))

    def snapshot(self):
        return {str(path.relative_to(self.root)): (path.read_bytes(), path.stat().st_mode & 0o777)
                for path in self.root.rglob("*") if path.is_file()}

    def report(self, cache=None):
        cache = cache or self.root / "custom cache"
        value = {"schemaVersion": 1, "kind": "loader-initialization", "version": "1.1.0",
                 "session": "00000000-0000-4000-8000-000000000001",
                 "startedAt": "2026-11-02T12:00:00.123456789Z", "completedAt": "2026-11-02T12:00:01Z",
                 "durationMs": 877, "gameVersion": "42.21", "javaVersion": "25",
                 "configDirectory": str(self.root / "config"), "debug": False,
                 "summary": {"loaded": 1, "serverLoaded": 0, "failed": 0, "disabled": 0, "pending": 0},
                 "mods": {"FreshMod": {"status": "loaded", "stage": "sources", "profile": "FreshMod.properties",
                                       "sourceCount": 1, "source": "/mods/FreshMod/media/lua/client/main.lua"}}}
        path = cache / "LuaBuddy/diagnostics.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")
        return cache, path, value

    def test_install_idempotence_profiles_status_remove(self):
        original_bytes = self.launcher.read_bytes()
        self.run_cli("install", "--mod", "AllInfo", "--mod", "AutoAll", "--mod", "FreshMod")
        config = self.current()
        self.assertEqual(config["vmArgs"][:-2], self.original["vmArgs"])
        self.assertEqual(config["vmArgs"][-1], CLI.AGENT)
        self.assertEqual(config["custom"], self.original["custom"])
        self.assertEqual((self.game / ".luabuddy/LuaBuddy.jar").read_bytes(), b"adapter fixture")
        self.assertEqual((self.root / "config/FreshMod.properties").read_text(), "id=FreshMod\n")
        self.assertEqual((self.root / "config/AllInfo.lua").read_bytes(), (REPO / "profiles/AllInfo.lua").read_bytes())
        backup = next((self.root / ".local/backups").glob("*.original"))
        self.assertEqual(backup.read_bytes(), original_bytes)
        (self.root / "config/AllInfo.properties").write_text("id=AllInfo\nenabled=false\n")
        (self.root / "config/AllInfo.lua").unlink()
        installed_bytes = self.launcher.read_bytes()
        stamp = self.launcher.stat().st_mtime_ns
        self.run_cli("install", "--mod", "AllInfo")
        self.assertEqual(self.launcher.read_bytes(), installed_bytes)
        self.assertEqual(self.launcher.stat().st_mtime_ns, stamp)
        self.assertIn("enabled=false", (self.root / "config/AllInfo.properties").read_text())
        self.assertTrue((self.root / "config/AllInfo.lua").is_file())
        self.assertEqual(backup.read_bytes(), original_bytes)
        result = self.run_cli("status")
        self.assertIn("LuaBuddy enabled", result.stdout)
        self.assertIn((self.root / "config").as_posix(), result.stdout)
        self.run_cli("remove")
        self.assertEqual(self.current(), self.original)
        self.assertTrue((self.root / "config/AllInfo.properties").exists())
        self.run_cli("remove")
        self.assertFalse(list(self.game.glob(".luabuddy-*")))

    def test_legacy_windows_argument_migration(self):
        self.original["vmArgs"] += [r"-Dpzclientmods.config=C:\Old Adapter\adapter.properties",
                                  r"-javaagent:C:\Old Adapter\ZombieBuddy.jar=patches_jar=C:\Old Adapter\PZClientMods.jar:localmods"]
        self.save(self.original)
        self.run_cli("install", "--mod", "FreshMod")
        args = self.current()["vmArgs"]
        self.assertEqual(args[:-2], self.original["vmArgs"][:-2])
        self.assertEqual(args[-1], CLI.AGENT)
        self.assertFalse(any("pzclientmods" in arg or "PZClientMods" in arg for arg in args))

    def test_unrelated_framework_refused_without_writes(self):
        for path in ("/framework/ZombieBuddy.jar", "ZombieBuddy.jar"):
            with self.subTest(path=path):
                config = dict(self.original, vmArgs=self.original["vmArgs"] + [f"-javaagent:{path}=patches_jar=Other.jar:other"])
                self.save(config)
                original = self.launcher.read_bytes()
                result = self.run_cli("install", "--mod", "AllInfo", success=False)
                self.assertIn("launcher unchanged", result.stderr)
                self.assertEqual(self.launcher.read_bytes(), original)
                self.assertFalse((self.game / ".luabuddy").exists())
                self.assertFalse((self.root / "config").exists())
                self.assertFalse((self.root / ".local").exists())

    def test_shared_framework_removal_refused_without_writes(self):
        self.original["vmArgs"] += [CLI.CONFIG_PREFIX + "/profiles",
                                   "-javaagent:ZombieBuddy.jar=patches_jar=.luabuddy/LuaBuddy.jar:luabuddy;Other.jar:other"]
        self.save(self.original)
        original = self.launcher.read_bytes()
        self.run_cli("remove", success=False)
        self.assertEqual(self.launcher.read_bytes(), original)

    def test_invalid_profile_or_vmargs_refused(self):
        original = self.launcher.read_bytes()
        self.run_cli("install", "--mod", "broken\nid", success=False)
        self.assertEqual(self.launcher.read_bytes(), original)
        self.assertFalse((self.root / "config").exists())
        self.original["vmArgs"] = [1]
        self.save(self.original)
        self.run_cli("install", success=False)
        self.assertFalse((self.game / ".luabuddy").exists())

    def test_custom_config_and_launcher_mode(self):
        directory = self.root / "custom profiles"
        self.launcher.chmod(0o640)
        self.run_cli("install", "--mod", "FreshMod", "--config-dir", str(directory))
        self.assertIn(CLI.CONFIG_PREFIX + directory.as_posix(), self.current()["vmArgs"])
        self.assertTrue((directory / "FreshMod.properties").is_file())
        if os.name != "nt":
            self.assertEqual(self.launcher.stat().st_mode & 0o777, 0o640)

    def test_arbitrary_mod_ids_use_portable_filenames(self):
        ids = ["Spiffo Knew [B42]", "GanydeBielovzki's Frockin Splendor! Vol.3", "CON", "NUL.txt", "../outside", "Trailing.", "路徑\\Example"]
        self.run_cli("install", *[arg for mod_id in ids for arg in ("--mod", mod_id)])
        profiles = list((self.root / "config").glob("*.properties"))
        self.assertEqual(len(profiles), len(ids))
        self.assertTrue(all(path.name.startswith("mod-") for path in profiles))
        values = {path.read_text(encoding="utf-8").removeprefix("id=").strip().replace("\\ ", " ").replace("\\\\", "\\") for path in profiles}
        self.assertEqual(values, set(ids))
        self.assertFalse((self.root / "outside.properties").exists())
        self.run_cli("install", "--mod", ids[0])
        self.assertEqual(len(list((self.root / "config").glob("*.properties"))), len(ids))

    def test_debug_toggle_preservation_idempotence_and_removal(self):
        self.original["vmArgs"].append(CLI.DEBUG_PROPERTY + "ger=unrelated")
        self.save(self.original)
        self.run_cli("install", "--debug", "--mod", "FreshMod")
        self.assertEqual([arg for arg in self.current()["vmArgs"] if CLI.debug_argument(arg)], ["-Dluabuddy.debug=true"])
        installed = self.launcher.read_bytes()
        stamp = self.launcher.stat().st_mtime_ns
        self.run_cli("install", "--debug")
        self.run_cli("install")
        self.assertEqual(self.launcher.read_bytes(), installed)
        self.assertEqual(self.launcher.stat().st_mtime_ns, stamp)
        self.assertIn("'true'", self.run_cli("status").stdout)
        self.run_cli("install", "--no-debug")
        self.assertFalse(any(CLI.debug_argument(arg) for arg in self.current()["vmArgs"]))
        config = self.current()
        config["vmArgs"].insert(0, "-Dluabuddy.debug=false")
        self.save(config)
        self.run_cli("install")
        self.assertIn("-Dluabuddy.debug=false", self.current()["vmArgs"])
        self.assertIn("'false'", self.run_cli("status").stdout)
        self.run_cli("remove")
        self.assertEqual(self.current(), self.original)

    def test_pending_profile_filename_collisions_refused_without_writes(self):
        for ids in (("Example", "example"), ("Mod with spaces", "mod-5ecee2a32e4f184f")):
            with self.subTest(ids=ids):
                before = self.snapshot()
                result = self.run_cli("install", "--mod", ids[0], "--mod", ids[1], success=False)
                self.assertIn("collide", result.stderr)
                self.assertEqual(self.snapshot(), before)
                self.assertFalse((self.root / "config").exists())
                self.assertFalse((self.game / ".luabuddy").exists())
                self.assertFalse((self.root / ".local").exists())

    def test_existing_profile_filename_collisions_refused(self):
        self.run_cli("install", "--mod", "Example", "--mod", "Mod with spaces")
        for mod_id in ("example", "mod-5ecee2a32e4f184f"):
            with self.subTest(mod_id=mod_id):
                before = self.snapshot()
                self.run_cli("install", "--mod", mod_id, success=False)
                self.assertEqual(self.snapshot(), before)
        (self.root / "config/Renamed.properties").write_text("id=AnotherMod\nenabled=false\n", encoding="utf-8")
        before = self.snapshot()
        self.run_cli("install", "--mod", "Renamed", success=False)
        self.assertEqual(self.snapshot(), before)
        (self.root / "config/Example.properties").rename(self.root / "config/ManuallyRenamed.properties")
        before = self.snapshot()
        self.run_cli("install", "--mod", "Example", success=False)
        self.assertEqual(self.snapshot(), before)

    def test_launcher_outside_selected_game_is_refused(self):
        for outside in (self.root / "OtherGame/ProjectZomboid64.json", self.game / "nested/ProjectZomboid64.json"):
            with self.subTest(launcher=outside):
                outside.parent.mkdir()
                outside.write_bytes(self.launcher.read_bytes())
                before = self.snapshot()
                result = self.run_cli("install", "--launcher", str(outside), success=False)
                self.assertIn("selected game directory", result.stderr)
                self.assertEqual(self.snapshot(), before)

    def test_noncanonical_existing_profile_ids_refuse_before_writes(self):
        config = self.root / "config"
        config.mkdir()
        path = config / "Renamed.properties"
        for text in ("id : Example\n", "id = Example\n", "id=Exam\\\nple\n", "\\u0069d=Example\n", "id=Other\nid : Example\n", "id=Other\n\\u0069d=Example\n"):
            with self.subTest(text=text):
                path.write_text(text, encoding="utf-8")
                before = self.snapshot()
                result = self.run_cli("install", "--mod", "Example", success=False)
                self.assertIn("Cannot safely read mod ID", result.stderr)
                self.assertEqual(self.snapshot(), before)

    def test_doctor_missing_report_is_read_only_and_bounded(self):
        self.original["unrelated_secret"] = "UNRELATED_LAUNCHER_CANARY"
        self.save(self.original)
        self.run_cli("install", "--mod", "FreshMod", "--debug")
        before = self.snapshot()
        cache = self.root / "never created cache"
        result = self.run_cli("doctor", "--cache-dir", str(cache), "--json",
                              environment={"UNRELATED_SECRET": "UNRELATED_ENV_CANARY"})
        value = json.loads(result.stdout)
        self.assertEqual(value["result"], "warning")
        self.assertIsNone(value["report"])
        self.assertEqual({check["name"]: check["status"] for check in value["checks"]}["last-initialization"], "warning")
        self.assertIn("'true'", result.stdout)
        self.assertNotIn("UNRELATED_LAUNCHER_CANARY", result.stdout + result.stderr)
        self.assertNotIn("UNRELATED_ENV_CANARY", result.stdout + result.stderr)
        self.assertIn("runtime behavior has not been verified", self.run_cli("doctor", "--cache-dir", str(cache)).stdout)
        self.assertEqual(self.snapshot(), before)
        self.assertFalse(cache.exists())

    def test_diagnostics_valid_report_without_game_discovery(self):
        cache, path, report = self.report()
        report["unrelated"] = "UNRELATED_REPORT_CANARY"
        report["mods"]["FreshMod"]["unrelated"] = "UNRELATED_MOD_CANARY"
        path.write_text(json.dumps(report), encoding="utf-8")
        before = self.snapshot()
        result = self.run_cli("diagnostics", "--cache-dir", str(cache), "--json",
                              environment={"PZ_GAME_DIR": str(self.root / "no game"), "UNRELATED_SECRET": "UNRELATED_ENV_CANARY"})
        expected = {key: value for key, value in report.items() if key != "unrelated"}
        expected["mods"] = {"FreshMod": {key: value for key, value in report["mods"]["FreshMod"].items() if key != "unrelated"}}
        self.assertEqual(json.loads(result.stdout), expected)
        self.assertNotIn("CANARY", result.stdout + result.stderr)
        text = self.run_cli("diagnostics", "--cache-dir", str(cache)).stdout
        for value in ("Last loader initialization: completed", report["session"], report["completedAt"], "FreshMod: loaded at sources", report["mods"]["FreshMod"]["source"], "later callbacks are not checked"):
            self.assertIn(value, text)
        self.assertEqual(self.snapshot(), before)

    def test_diagnostics_missing_and_invalid_reports_fail_without_traceback(self):
        cache = self.root / "missing cache"
        result = self.run_cli("diagnostics", "--cache-dir", str(cache), "--json", success=False)
        self.assertIn("LuaBuddy:", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertFalse(cache.exists())
        cache, path, report = self.report(cache)
        malformed = [b"not JSON", b"[]", b"null", b"{}", b'{"schemaVersion": true}', b"[" * 1500]
        for key, value in (("mods", []), ("summary", {}), ("debug", "false"), ("session", "bad uuid"),
                           ("completedAt", "not a date"), ("startedAt", "2026-13-02T12:00:00Z"), ("durationMs", float("nan")), ("traceback", [])):
            malformed.append(json.dumps(dict(report, **{key: value})).encode())
        malformed.append(json.dumps(dict(report, mods={"FreshMod": {"status": "healthy", "stage": "sources"}})).encode())
        malformed.append(json.dumps(dict(report, mods={"FreshMod": {"status": "loaded", "stage": "sources", "sourceCount": True}})).encode())
        malformed.append(b" " * (CLI.REPORT_LIMIT + 1))
        for contents in malformed:
            with self.subTest(contents=contents[:50]):
                path.write_bytes(contents)
                before = self.snapshot()
                result = self.run_cli("diagnostics", "--cache-dir", str(cache), "--json", success=False)
                self.assertIn("LuaBuddy:", result.stderr)
                self.assertNotIn("Traceback", result.stderr)
                self.assertEqual(self.snapshot(), before)

    def test_failed_initialization_reports_lua_and_java_tracebacks(self):
        self.run_cli("install", "--mod", "FreshMod")
        cache, path, report = self.report()
        report["summary"].update(loaded=0, failed=1)
        report["error"] = "Loader initialization failed"
        report["traceback"] = "java.io.IOException: root configuration fixture\n at fixture.Bootstrap.load"
        report["mods"]["FreshMod"].update(status="failed", stage="plugin", error="missing callback",
                                         traceback="fixture.lua:12: plugin failure\nstack traceback",
                                         javaTraceback="java.lang.RuntimeException: plugin failure\n at fixture.Loader.run")
        path.write_text(json.dumps(report), encoding="utf-8")
        before = self.snapshot()
        result = self.run_cli("diagnostics", "--cache-dir", str(cache), success=False)
        for text in ("Last loader initialization: failed", "FreshMod: failed at plugin", "missing callback", "fixture.lua:12", "java.lang.RuntimeException", "root configuration fixture"):
            self.assertIn(text, result.stdout)
        result = self.run_cli("diagnostics", "--cache-dir", str(cache), "--json", success=False)
        self.assertEqual(json.loads(result.stdout), report)
        value = json.loads(self.run_cli("doctor", "--cache-dir", str(cache), "--json", success=False).stdout)
        self.assertEqual(value["result"], "error")
        self.assertNotIn("mods", value["report"])
        self.assertEqual(self.snapshot(), before)

    def test_doctor_matching_report_and_stale_settings(self):
        self.run_cli("install", "--mod", "FreshMod")
        cache, path, report = self.report()
        before = self.snapshot()
        value = json.loads(self.run_cli("doctor", "--cache-dir", str(cache), "--json").stdout)
        self.assertEqual(value["result"], "ok")
        self.assertEqual(value["report"]["session"], report["session"])
        self.assertEqual(self.snapshot(), before)
        report.update(configDirectory=str(self.root / "other profiles"), debug=True)
        path.write_text(json.dumps(report), encoding="utf-8")
        value = json.loads(self.run_cli("doctor", "--cache-dir", str(cache), "--json").stdout)
        self.assertEqual(value["result"], "warning")
        self.assertIn("not runtime verified", next(item["message"] for item in value["checks"] if item["name"] == "report-configuration"))

    def test_default_cache_and_unverified_prepared_initialization(self):
        self.run_cli("install")
        home = self.root / "user home"
        _, path, report = self.report(home / "Zomboid")
        report["summary"].update(loaded=0, pending=1)
        report["mods"]["FreshMod"].update(status="prepared", stage="prepared")
        path.write_text(json.dumps(report), encoding="utf-8")
        environment = {"USERPROFILE" if os.name == "nt" else "HOME": str(home)}
        before = self.snapshot()
        self.assertIn("Last loader initialization: incomplete", self.run_cli("diagnostics", environment=environment).stdout)
        value = json.loads(self.run_cli("doctor", "--json", environment=environment).stdout)
        checks = {item["name"]: item["status"] for item in value["checks"]}
        self.assertEqual(value["result"], "warning")
        self.assertEqual(checks["last-initialization"], "warning")
        self.assertEqual(checks["profiles"], "warning")
        self.assertEqual(self.snapshot(), before)

    def test_json_diagnostics_support_unicode_on_ascii_stdout(self):
        cache, path, report = self.report()
        report["mods"]["路徑 Example"] = report["mods"].pop("FreshMod")
        path.write_text(json.dumps(report), encoding="utf-8")
        result = self.run_cli("diagnostics", "--cache-dir", str(cache), "--json", environment={"PYTHONIOENCODING": "ascii"})
        self.assertEqual(json.loads(result.stdout), report)
        self.assertIn("loaded at sources", self.run_cli("diagnostics", "--cache-dir", str(cache), environment={"PYTHONIOENCODING": "ascii"}).stdout)

    def test_doctor_runtime_config_and_report_errors_are_read_only(self):
        self.run_cli("install", "--mod", "FreshMod")
        cache, path, _ = self.report()
        (self.game / ".luabuddy/LuaBuddy.jar").write_bytes(b"wrong adapter")
        (self.game / ".luabuddy/ZombieBuddy.jar").unlink()
        shutil.rmtree(self.root / "config")
        path.write_text("{}", encoding="utf-8")
        before = self.snapshot()
        value = json.loads(self.run_cli("doctor", "--cache-dir", str(cache), "--json", success=False).stdout)
        checks = {item["name"]: item["status"] for item in value["checks"]}
        self.assertEqual(value["result"], "error")
        for name in ("LuaBuddy.jar", "ZombieBuddy.jar", "profiles", "last-initialization"):
            self.assertEqual(checks[name], "error")
        self.assertEqual(self.snapshot(), before)

    def test_doctor_invalid_launcher_and_unconfigured_install(self):
        cache = self.root / "unused cache"
        before = self.snapshot()
        value = json.loads(self.run_cli("doctor", "--cache-dir", str(cache), "--json", success=False).stdout)
        self.assertEqual(value["result"], "error")
        self.assertEqual(self.snapshot(), before)
        self.original["vmArgs"] = [42]
        self.save(self.original)
        value = json.loads(self.run_cli("doctor", "--cache-dir", str(cache), "--json", success=False).stdout)
        self.assertIn("launcher", {item["name"] for item in value["checks"] if item["status"] == "error"})

    @unittest.skipUnless(sys.platform.startswith("linux"), "requires Linux file-size limits")
    def test_backup_write_failure_is_atomic_and_retry_keeps_original(self):
        import resource
        import signal

        def limit_files():
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
            resource.setrlimit(resource.RLIMIT_FSIZE, (256, 256))

        self.original["large"] = "x" * 1024
        self.save(self.original)
        original = self.launcher.read_bytes()
        result = self.run_cli("install", success=False, preexec_fn=limit_files)
        self.assertNotIn("Traceback", result.stderr)
        self.assertEqual(self.launcher.read_bytes(), original)
        self.assertFalse(list(self.root.rglob("*.original")))
        self.assertFalse(list(self.root.rglob(".luabuddy-*")))
        self.run_cli("install")
        backup = next((self.root / ".local/backups").glob("*.original"))
        self.assertEqual(backup.read_bytes(), original)
        self.run_cli("install", "--debug")
        self.assertEqual(backup.read_bytes(), original)

    @unittest.skipUnless(sys.platform.startswith("linux"), "requires Linux file-size limits")
    def test_payload_preparation_failure_preserves_installed_files(self):
        import resource
        import signal

        def limit_files():
            signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
            resource.setrlimit(resource.RLIMIT_FSIZE, (256, 256))

        self.launcher.chmod(0o640)
        self.run_cli("install", "--mod", "FreshMod")
        (self.root / "dist/LuaBuddy.jar").write_bytes(b"new adapter")
        (self.root / "lib/ZombieBuddy.jar").write_bytes(b"x" * 1024)
        before = self.snapshot()
        self.run_cli("install", success=False, preexec_fn=limit_files)
        self.assertEqual(self.snapshot(), before)
        self.assertFalse(list(self.root.rglob(".luabuddy-*")))
        self.run_cli("install")
        self.assertEqual((self.game / ".luabuddy/LuaBuddy.jar").read_bytes(), b"new adapter")
        self.assertEqual((self.game / ".luabuddy/ZombieBuddy.jar").read_bytes(), b"x" * 1024)

    def test_replacement_failure_restores_installed_bytes_and_modes(self):
        self.run_cli("install", "--mod", "FreshMod")
        (self.root / "dist/LuaBuddy.jar").write_bytes(b"new adapter")
        (self.root / "lib/ZombieBuddy.jar").write_bytes(b"new framework")
        before = self.snapshot()
        replace = os.replace
        failed = False

        def refuse_framework(source, target):
            nonlocal failed
            if Path(target).name == "ZombieBuddy.jar" and not failed:
                failed = True
                raise PermissionError("fixture replacement failure")
            replace(source, target)

        with patch.object(CLI, "ROOT", self.root), patch.object(CLI.os, "replace", side_effect=refuse_framework):
            with self.assertRaises(PermissionError):
                CLI.configure(self.game, self.launcher, self.root / "config", [], False)
        self.assertTrue(failed)
        self.assertEqual(self.snapshot(), before)
        self.assertFalse(list(self.root.rglob(".luabuddy-*")))

    def test_environment_and_steam_library_discovery(self):
        with patch.dict(os.environ, {"PZ_GAME_DIR": str(self.game.parent)}):
            self.assertEqual(CLI.find_game(None), self.game.resolve())
        home = self.root / "home"
        steam = home / ".local/share/Steam/steamapps"
        steam.mkdir(parents=True)
        library = self.game.parents[3]
        escaped = str(library).replace("\\", "\\\\")
        (steam / "libraryfolders.vdf").write_text('"libraryfolders" { "1" { "path" "' + escaped + '" } }')
        environment = os.environ.copy()
        environment.pop("PZ_GAME_DIR", None)
        with patch.dict(os.environ, environment, clear=True), patch.object(Path, "home", return_value=home):
            self.assertEqual(CLI.find_game(None), self.game.resolve())


if __name__ == "__main__":
    unittest.main()
