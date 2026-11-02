#!/usr/bin/env python3
"""Run portable CLI checks and native Project Zomboid integration checks."""

import argparse
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def run(*command: str, cwd: Path = ROOT, env=None) -> None:
    subprocess.run(command, cwd=cwd, env=env, check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--game-dir", type=Path)
    args = parser.parse_args()
    run(sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py", "-v")
    build = [sys.executable, str(ROOT / "luabuddy.py"), "build"]
    if args.game_dir:
        build.extend(["--game-dir", str(args.game_dir)])
    run(*build)
    spec = importlib.util.spec_from_file_location("luabuddy", ROOT / "luabuddy.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    game = module.find_game(args.game_dir)
    executable = ".exe" if os.name == "nt" else ""
    javac = module.jdk_tool("javac")
    bundled_java = game / "jre64" / "bin" / ("java" + executable)
    java = str(bundled_java) if bundled_java.is_file() else shutil.which("java")
    if not java or not javac:
        raise SystemExit("The native checks need Java and javac 25 or newer.")
    with tempfile.TemporaryDirectory(prefix="luabuddy-check-") as temporary:
        scratch = Path(temporary)
        classes = scratch / "classes"
        classes.mkdir()
        classpath = os.pathsep.join(map(str, [game / "projectzomboid.jar", ROOT / "dist/LuaBuddy.jar", classes]))
        run(javac, "--release", "25", "-proc:none", "-Xlint:all", "-Werror", "-cp", classpath,
            "-d", str(classes), str(ROOT / "tests/EngineCheck.java"))
        env = os.environ.copy()
        env["LD_LIBRARY_PATH"] = os.pathsep.join([str(game / "natives"), str(game / "jre64/lib"), env.get("LD_LIBRARY_PATH", "")])
        run(java, "--enable-native-access=ALL-UNNAMED", "-Dzomboid.steam=0", "-Djava.library.path=natives",
            "-cp", classpath, "luabuddy.EngineCheck", str(ROOT), str(scratch / "fixtures"), cwd=game, env=env)
    luajit = shutil.which("luajit")
    if luajit:
        run(luajit, "tests/loader.lua", "resources/bootstrap.lua")
    else:
        print("SKIP: extra Lua failure checks need LuaJIT; native integration checks passed.")


if __name__ == "__main__":
    main()
