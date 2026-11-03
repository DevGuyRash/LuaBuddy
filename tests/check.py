#!/usr/bin/env python3
"""Run portable CLI checks and native Project Zomboid integration checks."""

import argparse
from pathlib import Path
import shutil
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]


def run(*command: str, cwd: Path = ROOT, env=None) -> None:
    subprocess.run(command, cwd=cwd, env=env, check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--game-dir", type=Path)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    run(sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py", "-v")
    build = [sys.executable, str(ROOT / "luabuddy.py"), "compatibility"]
    if args.game_dir:
        build.extend(["--game-dir", str(args.game_dir)])
    if args.offline:
        build.append("--offline")
    run(*build)
    luajit = shutil.which("luajit")
    if luajit:
        run(luajit, "tests/loader.lua", "resources/bootstrap.lua")
    else:
        print("SKIP: extra Lua failure checks need LuaJIT; native integration checks passed.")


if __name__ == "__main__":
    main()
