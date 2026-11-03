from io import StringIO
import json
from pathlib import Path
import shutil
import sys
import unittest
from unittest.mock import patch

import test_cli


CLI = test_cli.CLI


class Output(StringIO):
    def reconfigure(self, **kwargs):
        pass


class UpdaterCheck(unittest.TestCase):
    def setUp(self):
        self.fixture = test_cli.CliCheck()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        self.game = self.fixture.game
        self.launcher = self.fixture.launcher
        self.dependency = self.fixture.dependency
        root = patch.object(CLI, "ROOT", self.root)
        root.start()
        self.addCleanup(root.stop)

    def main(self, *arguments):
        output, errors = Output(), Output()
        with patch.object(sys, "argv", ["luabuddy.py", *arguments, "--game-dir", str(self.game)]), \
                patch.object(sys, "stdout", output), patch.object(sys, "stderr", errors):
            result = CLI.main()
        return result, output.getvalue(), errors.getvalue()

    def candidate(self, version="2.3.5", asset_id=2, extra=b""):
        data = test_cli.framework_jar(version, extra)
        tag = "v" + version
        release = {"tag": tag, "asset_id": asset_id, "size": len(data),
                   "sha256": CLI.hashlib.sha256(data).hexdigest(),
                   "url": f"{CLI.zombiebuddy.REPOSITORY}/releases/download/{tag}/ZombieBuddy.jar",
                   "release_url": f"{CLI.zombiebuddy.REPOSITORY}/releases/tag/{tag}"}
        return release, data

    def probe(self, version="2.3.5", helper=b"new verified helper"):
        return helper, {"gameVersion": "42.21", "zombieBuddyVersion": version}

    def stale(self):
        path = CLI.dependency_directory() / "current.json"
        value = json.loads(path.read_bytes())
        value["checkedAt"] = "2000-01-01T00:00:00Z"
        path.write_bytes(CLI.json_bytes(value))

    def installed(self):
        self.assertEqual(self.main("install")[0], 0)

    def test_cold_build_downloads_then_install_reuses_verified_pair(self):
        shutil.rmtree(CLI.dependency_directory())
        shutil.rmtree(self.root / "dist")
        release, data = self.candidate()
        before_launcher = self.launcher.read_bytes()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release) as fetch, \
                patch.object(CLI.zombiebuddy, "download_release", return_value=data) as download, \
                patch.object(CLI, "compile_and_verify", return_value=self.probe()) as probe:
            self.assertEqual(self.main("build")[0], 0)
            self.assertEqual(self.launcher.read_bytes(), before_launcher)
            self.assertFalse((self.game / ".luabuddy").exists())
            self.assertEqual((self.root / "dist/LuaBuddy.jar").read_bytes(), b"new verified helper")
            self.assertEqual(CLI.cached_dependency().release, release)
            self.assertEqual(CLI.cached_dependency().data, data)
            self.assertEqual(self.main("install")[0], 0)
            fetch.assert_called_once_with()
            download.assert_called_once_with(release)
            probe.assert_called_once()
        self.assertEqual((self.game / ".luabuddy/ZombieBuddy.jar").read_bytes(), data)
        self.assertEqual(json.loads((self.game / ".luabuddy/ZombieBuddy.release.json").read_bytes()), release)

    def test_fresh_verified_cache_requires_no_network_or_compiler(self):
        with patch.object(CLI.zombiebuddy, "fetch_latest", side_effect=AssertionError("network")), \
                patch.object(CLI, "compile_and_verify", side_effect=AssertionError("compiler")):
            self.assertEqual(self.main("install")[0], 0)
            self.assertEqual(self.main("build", "--offline")[0], 0)
        self.assertEqual((self.game / ".luabuddy/ZombieBuddy.jar").read_bytes(), self.dependency.data)

    def test_stale_unchanged_release_checks_once_and_reuses_build(self):
        self.stale()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=self.dependency.release) as fetch, \
                patch.object(CLI.zombiebuddy, "download_release", side_effect=AssertionError("download")), \
                patch.object(CLI, "compile_and_verify", side_effect=AssertionError("compiler")):
            self.assertEqual(self.main("install")[0], 0)
            self.assertEqual(self.main("install")[0], 0)
            fetch.assert_called_once_with()
        self.assertNotEqual(CLI.cached_dependency().checked_at, "2000-01-01T00:00:00Z")

    def test_automatic_update_promotes_pair_after_compatibility_passes(self):
        self.installed()
        self.stale()
        release, data = self.candidate()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release), \
                patch.object(CLI.zombiebuddy, "download_release", return_value=data), \
                patch.object(CLI, "compile_and_verify", return_value=self.probe()) as probe:
            self.assertEqual(self.main("install")[0], 0)
            probe.assert_called_once()
        self.assertEqual(CLI.cached_dependency().release, release)
        self.assertEqual((self.game / ".luabuddy/ZombieBuddy.jar").read_bytes(), data)
        self.assertEqual((self.game / ".luabuddy/LuaBuddy.jar").read_bytes(), b"new verified helper")

    def test_explicit_update_preserves_profiles_debug_and_original_backup(self):
        custom = self.root / "selected profiles"
        self.assertEqual(self.main("install", "--config-dir", str(custom), "--mod", "FreshMod", "--debug")[0], 0)
        profile = custom / "FreshMod.properties"
        profile.write_text("id=FreshMod\nenabled=false\n")
        backup = next((self.root / ".local/backups").glob("*.original"))
        old_backup = backup.read_bytes()
        old_launcher = self.launcher.read_bytes()
        release, data = self.candidate()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release) as fetch, \
                patch.object(CLI.zombiebuddy, "download_release", return_value=data) as download, \
                patch.object(CLI, "compile_and_verify", return_value=self.probe()) as probe:
            self.assertEqual(self.main("update")[0], 0)
            stamp = (self.game / ".luabuddy/ZombieBuddy.jar").stat().st_mtime_ns
            self.assertEqual(self.main("update")[0], 0)
            self.assertEqual(fetch.call_count, 2)
            download.assert_called_once_with(release)
            probe.assert_called_once()
        self.assertEqual(self.launcher.read_bytes(), old_launcher)
        self.assertEqual(backup.read_bytes(), old_backup)
        self.assertEqual(profile.read_text(), "id=FreshMod\nenabled=false\n")
        self.assertEqual((self.game / ".luabuddy/ZombieBuddy.jar").stat().st_mtime_ns, stamp)

    def test_update_check_compares_actual_installed_hash_without_writes(self):
        self.installed()
        for release, expected in ((self.dependency.release, 0), (self.candidate()[0], 2),
                                  (self.candidate("2.3.4", 3, b"rebuilt")[0], 2)):
            with self.subTest(release=release["sha256"]):
                before = self.fixture.snapshot()
                with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release) as fetch, \
                        patch.object(CLI.zombiebuddy, "download_release", side_effect=AssertionError("download")), \
                        patch.object(CLI, "compile_and_verify", side_effect=AssertionError("compiler")):
                    result, output, error = self.main("update", "--check")
                    self.assertEqual(result, expected, output + error)
                    self.assertIn("SHA-256", output)
                    fetch.assert_called_once_with()
                self.assertEqual(self.fixture.snapshot(), before)

    def test_check_works_without_dependency_cache_and_reports_missing_install(self):
        shutil.rmtree(self.root / ".local")
        before = self.fixture.snapshot()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=self.dependency.release):
            result, output, _ = self.main("update", "--check")
        self.assertEqual(result, 2)
        self.assertIn("not installed", output)
        self.assertEqual(self.fixture.snapshot(), before)
        self.assertFalse((self.root / ".local").exists())

    def test_offline_missing_corrupt_or_unmatched_cache_fails_without_changes(self):
        self.installed()
        selection = CLI.dependency_directory() / "current.json"
        directory = selection.parent / str(self.dependency.release["asset_id"])
        jar = directory / "ZombieBuddy.jar"
        metadata = directory / "release.json"
        originals = {path: path.read_bytes() for path in (selection, jar, metadata)}
        mutations = ((selection, None), (selection, b"{}"), (jar, None), (jar, b"corrupt"),
                     (metadata, b"{}"), (selection, CLI.json_bytes({
                         "schemaVersion": 1, "release": self.dependency.release, "checkedAt": "tomorrow"})))
        with patch.object(CLI.zombiebuddy, "fetch_latest", side_effect=AssertionError("network")), \
                patch.object(CLI, "compile_and_verify", side_effect=AssertionError("compiler")):
            for path, contents in mutations:
                with self.subTest(path=path, contents=contents):
                    if contents is None:
                        path.unlink()
                    else:
                        path.write_bytes(contents)
                    before = self.fixture.snapshot()
                    result, _, errors = self.main("install", "--offline")
                    self.assertEqual(result, 1)
                    self.assertIn("No verified cached", errors)
                    self.assertEqual(self.fixture.snapshot(), before)
                    path.write_bytes(originals[path])

    def test_automatic_connection_failure_uses_verified_cache_with_warning(self):
        self.installed()
        self.stale()
        for download_fails in (False, True):
            with self.subTest(download_fails=download_fails):
                release, _ = self.candidate()
                failure = CLI.zombiebuddy.ZombieBuddyNetworkError("Fixture connection failed")
                before = self.fixture.snapshot()
                with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release,
                                  side_effect=None if download_fails else failure), \
                        patch.object(CLI.zombiebuddy, "download_release", side_effect=failure):
                    result, _, error = self.main("install")
                self.assertEqual(result, 0, error)
                self.assertIn("latest release not checked", error)
                self.assertEqual(self.fixture.snapshot(), before)

    def test_explicit_transport_and_integrity_failures_preserve_every_file(self):
        self.installed()
        release, _ = self.candidate()
        for phase, failure in (("fetch_latest", CLI.zombiebuddy.ZombieBuddyNetworkError("Fixture offline")),
                               ("fetch_latest", CLI.zombiebuddy.ZombieBuddyError("Fixture invalid metadata")),
                               ("download_release", CLI.zombiebuddy.ZombieBuddyNetworkError("Fixture interrupted download")),
                               ("download_release", CLI.zombiebuddy.ZombieBuddyError("Fixture digest mismatch"))):
            with self.subTest(phase=phase, failure=failure):
                before = self.fixture.snapshot()
                with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release), \
                        patch.object(CLI.zombiebuddy, phase, side_effect=failure), \
                        patch.object(CLI, "compile_and_verify", side_effect=AssertionError("compiler")):
                    self.assertEqual(self.main("update")[0], 1)
                self.assertEqual(self.fixture.snapshot(), before)
                self.assertFalse((CLI.dependency_directory() / str(release["asset_id"])).exists())

    def test_automatic_integrity_failure_does_not_fall_back_to_cached_version(self):
        self.installed()
        self.stale()
        before = self.fixture.snapshot()
        with patch.object(CLI.zombiebuddy, "fetch_latest", side_effect=CLI.zombiebuddy.ZombieBuddyError("Fixture invalid publisher metadata")):
            result, _, errors = self.main("install")
        self.assertEqual(result, 1)
        self.assertIn("invalid publisher metadata", errors)
        self.assertNotIn("Using verified cached", errors)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_update_check_rejects_corrupt_installed_jar_without_writes(self):
        self.installed()
        (self.game / ".luabuddy/ZombieBuddy.jar").write_bytes(b"partial JAR")
        before = self.fixture.snapshot()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=self.dependency.release):
            result, _, errors = self.main("update", "--check")
        self.assertEqual(result, 1)
        self.assertIn("not a JAR", errors)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_bad_download_is_rejected_before_compilation_and_promotion(self):
        self.installed()
        release, _ = self.candidate()
        before = self.fixture.snapshot()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release), \
                patch.object(CLI.zombiebuddy, "download_release", return_value=b"partial JAR"), \
                patch.object(CLI, "compile_and_verify", side_effect=AssertionError("compiler")):
            self.assertEqual(self.main("update")[0], 1)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_incompatible_candidate_never_replaces_working_pair_or_selection(self):
        self.installed()
        release, data = self.candidate()
        before = self.fixture.snapshot()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release), \
                patch.object(CLI.zombiebuddy, "download_release", return_value=data), \
                patch.object(CLI, "compile_and_verify", side_effect=ValueError("Fixture native API incompatible")):
            result, _, error = self.main("update")
        self.assertEqual(result, 1)
        self.assertIn("incompatible", error)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_existing_asset_metadata_drift_is_not_silently_replaced(self):
        self.installed()
        release, data = self.candidate("2.3.4", self.dependency.release["asset_id"], b"changed publisher bytes")
        before = self.fixture.snapshot()
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release), \
                patch.object(CLI.zombiebuddy, "download_release", return_value=data) as download:
            result, _, error = self.main("update")
            download.assert_not_called()
        self.assertEqual(result, 1)
        self.assertIn("metadata changed", error)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_corrupt_cached_bytes_are_repaired_only_from_verified_download(self):
        self.installed()
        jar = CLI.dependency_directory() / str(self.dependency.release["asset_id"]) / "ZombieBuddy.jar"
        jar.write_bytes(b"corrupt")
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=self.dependency.release), \
                patch.object(CLI.zombiebuddy, "download_release", return_value=self.dependency.data) as download, \
                patch.object(CLI, "compile_and_verify", side_effect=AssertionError("compiler")):
            self.assertEqual(self.main("install")[0], 0)
            download.assert_called_once_with(self.dependency.release)
        self.assertEqual(jar.read_bytes(), self.dependency.data)

    def test_changed_game_or_build_artifact_requires_native_gate(self):
        self.installed()
        for path in (self.game / "projectzomboid.jar", self.root / "dist/LuaBuddy.jar"):
            with self.subTest(path=path):
                original = path.read_bytes()
                path.write_bytes(b"changed fixture")
                before = self.fixture.snapshot()
                with patch.object(CLI, "compile_and_verify", side_effect=ValueError("native gate refused")) as probe:
                    self.assertEqual(self.main("install", "--offline")[0], 1)
                    probe.assert_called_once()
                self.assertEqual(self.fixture.snapshot(), before)
                path.write_bytes(original)

    def test_changed_java_runtime_requires_native_gate_without_game_changes(self):
        executable = self.game / "jre64/bin" / ("java.exe" if CLI.os.name == "nt" else "java")
        executable.parent.mkdir(parents=True)
        executable.write_bytes(b"previous Java launcher")
        release = executable.parent.parent / "release"
        release.write_text('JAVA_VERSION="25.0.1"\n', encoding="utf-8")
        record_path = self.root / "dist/LuaBuddy.build.json"
        record = json.loads(record_path.read_text())
        record.update(CLI.build_inputs(self.game, self.dependency))
        record_path.write_bytes(CLI.json_bytes(record))
        game_bytes = (self.game / "projectzomboid.jar").read_bytes()
        for path, replacement in ((release, b'JAVA_VERSION="27.0.1"\n'), (executable, b"changed Java launcher")):
            with self.subTest(runtime_file=path.name):
                original = path.read_bytes()
                path.write_bytes(replacement)
                before = self.fixture.snapshot()
                with patch.object(CLI, "compile_and_verify", side_effect=ValueError("new runtime is incompatible")) as probe:
                    self.assertEqual(self.main("install", "--offline")[0], 1)
                    probe.assert_called_once()
                self.assertEqual((self.game / "projectzomboid.jar").read_bytes(), game_bytes)
                self.assertEqual(self.fixture.snapshot(), before)
                path.write_bytes(original)

    def test_compatibility_checks_candidate_without_publishing_build_or_selection(self):
        self.installed()
        self.stale()
        release, data = self.candidate()
        protected = {path: path.read_bytes() for path in (self.launcher, self.root / "dist/LuaBuddy.jar",
                     self.root / "dist/LuaBuddy.build.json", CLI.dependency_directory() / "current.json",
                     self.game / ".luabuddy/LuaBuddy.jar", self.game / ".luabuddy/ZombieBuddy.jar")}
        with patch.object(CLI.zombiebuddy, "fetch_latest", return_value=release), \
                patch.object(CLI.zombiebuddy, "download_release", return_value=data), \
                patch.object(CLI, "compile_and_verify", return_value=self.probe()) as probe:
            result, output, error = self.main("compatibility")
            self.assertEqual(result, 0, error)
            self.assertIn("instrumented OnGameStart", output)
            probe.assert_called_once()
        self.assertEqual({path: path.read_bytes() for path in protected}, protected)
        self.assertEqual((CLI.dependency_directory() / str(release["asset_id"]) / "ZombieBuddy.jar").read_bytes(), data)

    def test_doctor_and_remove_work_offline_without_cache(self):
        self.installed()
        cache, _, _ = self.fixture.report()
        shutil.rmtree(CLI.dependency_directory())
        before = self.fixture.snapshot()
        with patch.object(CLI.zombiebuddy, "fetch_latest", side_effect=AssertionError("network")):
            result, output, error = self.main("doctor", "--cache-dir", str(cache), "--json")
            self.assertEqual(result, 0, error)
            framework = next(check for check in json.loads(output)["checks"] if check["name"] == "ZombieBuddy.jar")
            self.assertEqual(framework["status"], "warning")
            self.assertIn(self.dependency.release["sha256"], framework["message"])
            self.assertEqual(self.fixture.snapshot(), before)
            self.assertEqual(self.main("remove")[0], 0)
        self.assertEqual(self.fixture.current(), self.fixture.original)
        self.assertFalse(CLI.dependency_directory().exists())

    def test_split_preview_layout_is_discovered_and_included_in_fingerprint(self):
        (self.game / "projectzomboid.jar").unlink()
        java = self.game / "java"
        java.mkdir()
        (java / "projectzomboid.jar").write_bytes(b"preview game")
        library = java / "dependency.jar"
        library.write_bytes(b"preview library")
        self.assertEqual(CLI.find_game(self.game.parent), self.game)
        self.assertEqual(CLI.game_jars(self.game), [library, java / "projectzomboid.jar"])
        before = CLI.build_inputs(self.game, self.dependency)
        library.write_bytes(b"changed preview library")
        self.assertNotEqual(CLI.build_inputs(self.game, self.dependency)["game"], before["game"])

    def test_native_gate_uses_candidate_agent_and_cleans_game_scratch(self):
        moved = self.game.with_name("Project=Zomboid")
        self.game.rename(moved)
        self.game = moved
        source = self.root / "src/luabuddy/Probe.java"
        source.parent.mkdir(parents=True)
        source.write_text("class Probe {}")
        resource = self.root / "resources/bootstrap.lua"
        resource.parent.mkdir()
        resource.write_text("return {}")
        check = self.root / "tests/EngineCheck.java"
        check.parent.mkdir()
        check.write_text("class EngineCheck {}")
        calls = []

        def execute(arguments, **kwargs):
            calls.append((arguments, kwargs))
            self.assertEqual(kwargs["cwd"], self.game)
            self.assertGreater(kwargs["timeout"], 0)
            if "--create" in arguments:
                path = self.game / arguments[arguments.index("--file") + 1]
                path.write_bytes(b"candidate helper")

        with patch.object(CLI, "jdk_tool", side_effect=lambda name: name), \
                patch.object(CLI.subprocess, "run", side_effect=execute), \
                patch.object(CLI, "read_report", return_value={
                    "gameVersion": "42.21", "zombieBuddyVersion": "2.3.4",
                    "summary": {"failed": 0, "pending": 0, "loaded": 1}, "mods": {}}):
            helper, report = CLI.compile_and_verify(self.game, self.dependency)
        self.assertEqual(helper, b"candidate helper")
        self.assertEqual(report["zombieBuddyVersion"], "2.3.4")
        self.assertEqual(len(calls), 4)
        arguments, kwargs = calls[-1]
        self.assertIn("-Dluabuddy.verifyAgent=true", arguments)
        agent = next(argument for argument in arguments if argument.startswith("-javaagent:"))
        agent_path = agent.removeprefix("-javaagent:").partition("=")[0]
        self.assertFalse(Path(agent_path).is_absolute())
        self.assertNotIn("=", agent_path)
        options = agent.partition("=")[2]
        patch_option, config_option = options.split(",")
        patch_path = patch_option.removeprefix("patches_jar=").removesuffix(":luabuddy")
        config_path = config_option.removeprefix("config_dir=")
        self.assertFalse(Path(patch_path).is_absolute())
        self.assertFalse(Path(config_path).is_absolute())
        self.assertNotIn(":", patch_path + config_path)
        self.assertNotIn(",", patch_path + config_path)
        classpath = arguments[arguments.index("-cp") + 1].split(CLI.os.pathsep)
        self.assertIn(".", classpath)
        self.assertIn("projectzomboid.jar", classpath)
        self.assertFalse(list(self.game.glob(".luabuddy-check-*")))

    def test_native_gate_timeout_logs_failure_without_leaving_scratch(self):
        for name in ("src/luabuddy/Probe.java", "resources/bootstrap.lua", "tests/EngineCheck.java"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()
        with patch.object(CLI, "jdk_tool", return_value="fixture-javac"), \
                patch.object(CLI.subprocess, "run", side_effect=CLI.subprocess.TimeoutExpired("fixture", 180)):
            with self.assertRaisesRegex(ValueError, "Helper compilation timed out"):
                CLI.compile_and_verify(self.game, self.dependency)
        self.assertIn("Helper compilation", (self.root / ".local/compatibility.log").read_text())
        self.assertFalse(list(self.game.glob(".luabuddy-check-*")))


if __name__ == "__main__":
    unittest.main()
