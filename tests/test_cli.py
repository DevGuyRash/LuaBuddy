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
        self.root = Path(self.scratch.name)
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

    def run_cli(self, *args, success=True):
        env = os.environ.copy()
        env.pop("PZ_GAME_DIR", None)
        result = subprocess.run([sys.executable, str(self.root / "luabuddy.py"), args[0],
                                 "--game-dir", str(self.game.parent), *args[1:]],
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, timeout=20)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def current(self):
        return json.loads(self.launcher.read_text(encoding="utf-8"))

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
