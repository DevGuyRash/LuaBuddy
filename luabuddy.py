#!/usr/bin/env python3
"""Build and install LuaBuddy for Project Zomboid on Linux and Windows."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent
AGENT = "-javaagent:.luabuddy/ZombieBuddy.jar=patches_jar=.luabuddy/LuaBuddy.jar:luabuddy"
CONFIG_PREFIX = "-Dluabuddy.config="
LEGACY_PREFIX = "-Dpzclientmods.config="


def game_directory(path: Path) -> Path | None:
    for candidate in (path, path / "projectzomboid"):
        if (candidate / "projectzomboid.jar").is_file():
            return candidate.resolve()
    return None


def steam_libraries() -> list[Path]:
    home = Path.home()
    roots = [home / ".local/share/Steam", home / ".steam/steam",
             home / ".var/app/com.valvesoftware.Steam/.local/share/Steam"]
    for key in ("ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"):
        if os.environ.get(key):
            roots.append(Path(os.environ[key]) / "Steam")
    if os.name == "nt":
        import winreg
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Valve\Steam") as key:
                roots.append(Path(winreg.QueryValueEx(key, "SteamPath")[0]))
        except OSError:
            pass
    libraries = set(roots)
    for root in roots:
        vdf = root / "steamapps/libraryfolders.vdf"
        if vdf.is_file():
            for value in re.findall(r'"path"\s*"((?:\\.|[^"\\])*)"', vdf.read_text(encoding="utf-8")):
                libraries.add(Path(value.replace("\\\\", "\\").replace('\\"', '"')))
    return sorted(libraries, key=str)


def find_game(explicit: Path | None) -> Path:
    configured = explicit or (Path(os.environ["PZ_GAME_DIR"]) if os.environ.get("PZ_GAME_DIR") else None)
    if configured is not None:
        result = game_directory(configured.expanduser())
        if result is None:
            raise ValueError(f"No projectzomboid.jar under {configured}")
        return result
    found = {game for library in steam_libraries()
             if (game := game_directory(library / "steamapps/common/ProjectZomboid")) is not None}
    if len(found) != 1:
        raise ValueError("Specify --game-dir or PZ_GAME_DIR; Steam discovery found "
                         + (", ".join(map(str, sorted(found))) or "no game installation"))
    return found.pop()


def jdk_tool(name: str) -> str:
    suffix = ".exe" if os.name == "nt" else ""
    if os.environ.get("JAVA_HOME"):
        candidate = Path(os.environ["JAVA_HOME"]) / "bin" / (name + suffix)
        if candidate.is_file():
            return str(candidate)
    result = shutil.which(name)
    if result is None:
        raise ValueError(f"JDK 25 or newer is required; put {name} on PATH or set JAVA_HOME")
    return result


def atomic_write(path: Path, data: bytes) -> bool:
    if path.is_file() and path.read_bytes() == data:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    mode = stat.S_IMODE(path.stat().st_mode) if path.exists() else None
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".luabuddy-", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(data)
        if mode is not None:
            temporary.chmod(mode)
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return True


def build(game: Path) -> None:
    sources = sorted((ROOT / "src/luabuddy").glob("*.java"))
    framework = ROOT / "lib/ZombieBuddy.jar"
    if not sources or not framework.is_file():
        raise ValueError("Missing Java sources or lib/ZombieBuddy.jar")
    destination = ROOT / "dist/LuaBuddy.jar"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="luabuddy-build-") as directory:
        scratch = Path(directory)
        classes = scratch / "classes"
        classes.mkdir()
        subprocess.run([jdk_tool("javac"), "--release", "25", "-proc:none", "-Xlint:all", "-Werror",
                        "-cp", os.pathsep.join(map(str, [game / "projectzomboid.jar", framework])),
                        "-d", str(classes), *map(str, sources)], check=True)
        artifact = scratch / "LuaBuddy.jar"
        subprocess.run([jdk_tool("jar"), "--create", "--file", str(artifact),
                        "-C", str(classes), ".", "-C", str(ROOT / "resources"), "."], check=True)
        atomic_write(destination, artifact.read_bytes())
    print(f"Built {destination}")


def buddy_agent(argument: str) -> bool:
    if not argument.startswith("-javaagent:"):
        return False
    path = argument[len("-javaagent:"):].split("=", 1)[0].replace("\\", "/")
    return path.rsplit("/", 1)[-1].lower() == "zombiebuddy.jar"


def owned_agent(argument: str, legacy: bool = False) -> bool:
    if not buddy_agent(argument) or "=" not in argument:
        return False
    options = argument.split("=", 1)[1]
    if not options.startswith("patches_jar=") or "," in options or ";" in options:
        return False
    entry = options[len("patches_jar="):].replace("\\", "/")
    name = "PZClientMods.jar:localmods" if legacy else "LuaBuddy.jar:luabuddy"
    return entry == name or entry.endswith("/" + name)


def launcher_data(launcher: Path) -> tuple[bytes, dict, list[str]]:
    original = launcher.read_bytes()
    config = json.loads(original)
    args = config.get("vmArgs") if isinstance(config, dict) else None
    if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
        raise ValueError("Launcher vmArgs must be a list of strings")
    return original, config, args


def profile_files(ids: list[str], directory: Path) -> dict[Path, bytes]:
    files = {}
    for mod_id in ids:
        if not mod_id.strip() or any(ord(char) < 32 or ord(char) == 127 for char in mod_id):
            raise ValueError("Mod IDs must be nonempty and contain no control characters")
        reserved = re.fullmatch(r"CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9]", mod_id.split(".", 1)[0], re.IGNORECASE)
        portable = re.fullmatch(r"[A-Za-z0-9_.-]+", mod_id) and not reserved and not mod_id.endswith(".") and len(mod_id) <= 180
        stem = mod_id if portable else "mod-" + hashlib.sha256(mod_id.encode("utf-8")).hexdigest()[:16]
        source = ROOT / "profiles" / (stem + ".properties")
        target = directory / source.name
        if not target.exists():
            escaped_id = mod_id.replace("\\", "\\\\").replace(" ", "\\ ")
            files[target] = source.read_bytes() if source.is_file() else f"id={escaped_id}\n".encode("utf-8")
        plugin = ROOT / "profiles" / (stem + ".lua")
        if source.is_file() and plugin.is_file() and not (directory / plugin.name).exists():
            files[directory / plugin.name] = plugin.read_bytes()
    return files


def configure(game: Path, launcher: Path, directory: Path, ids: list[str], remove: bool) -> None:
    original, config, args = launcher_data(launcher)
    if remove and any(buddy_agent(arg) and "LuaBuddy.jar:luabuddy" in arg and not owned_agent(arg) for arg in args):
        raise ValueError("LuaBuddy shares a ZombieBuddy agent with other options. Remove its patch manually; launcher unchanged.")
    updated = [arg for arg in args if not arg.startswith(CONFIG_PREFIX) and not owned_agent(arg)]
    pending = {}
    if not remove:
        updated = [arg for arg in updated if not arg.startswith(LEGACY_PREFIX) and not owned_agent(arg, legacy=True)]
        if any(buddy_agent(arg) for arg in updated):
            raise ValueError("Another ZombieBuddy agent or patch configuration is present. Combine it manually before installing LuaBuddy; launcher unchanged.")
        payload = {game / ".luabuddy/LuaBuddy.jar": ROOT / "dist/LuaBuddy.jar",
                   game / ".luabuddy/ZombieBuddy.jar": ROOT / "lib/ZombieBuddy.jar"}
        for source in payload.values():
            if not source.is_file():
                raise ValueError(f"Missing {source}; run luabuddy.py build first")
        pending = profile_files(ids, directory)
        config_path = directory.as_posix()
        if "\n" in config_path or "\r" in config_path:
            raise ValueError("The profile directory must not contain a line break")
        updated.extend([CONFIG_PREFIX + config_path, AGENT])
    changed = updated != args
    backup = ROOT / ".local/backups" / (hashlib.sha256(str(launcher).encode()).hexdigest()[:16] + "-" + launcher.name + ".original")
    if changed:
        backup.parent.mkdir(parents=True, exist_ok=True)
        if not backup.exists():
            with backup.open("xb") as stream:
                stream.write(original)
    if not remove:
        for target, source in payload.items():
            atomic_write(target, source.read_bytes())
        directory.mkdir(parents=True, exist_ok=True)
        for target, contents in pending.items():
            atomic_write(target, contents)
    if changed:
        config["vmArgs"] = updated
        atomic_write(launcher, (json.dumps(config, indent=2, ensure_ascii=False) + "\n").encode("utf-8"))
    print(("Updated " if changed else "Already configured ") + str(launcher))
    if changed:
        print(f"Original launcher backup: {backup}")
    if remove:
        print(f"Runtime files retained at {game / '.luabuddy'}; profiles retained at {directory}")
    else:
        print(f"Profiles: {directory}")
    print("Changes apply to the next normal Steam launch. Running sessions are unaffected.")


def status(game: Path, launcher: Path) -> None:
    _, _, args = launcher_data(launcher)
    enabled = any(owned_agent(arg) for arg in args)
    print(f"LuaBuddy {'enabled' if enabled else 'not enabled'} in {launcher}")
    for arg in args:
        if arg.startswith(CONFIG_PREFIX) or owned_agent(arg):
            print(arg)
    for name in ("LuaBuddy.jar", "ZombieBuddy.jar"):
        path = game / ".luabuddy" / name
        print(f"Runtime {'present' if path.is_file() else 'missing'}: {path}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    for name in ("build", "install", "remove", "status"):
        command = subparsers.add_parser(name)
        command.add_argument("--game-dir", type=Path)
        command.add_argument("--launcher", type=Path)
        if name in ("install", "remove"):
            command.add_argument("--config-dir", type=Path, default=ROOT / "config")
        if name == "install":
            command.add_argument("--mod", action="append", default=[])
    args = parser.parse_args()
    try:
        game = find_game(args.game_dir or (args.launcher.parent if args.launcher else None))
        launcher = args.launcher.resolve() if args.launcher else game / "ProjectZomboid64.json"
        if args.command == "build":
            build(game)
        elif args.command == "status":
            status(game, launcher)
        else:
            configure(game, launcher, args.config_dir.expanduser().resolve(),
                      args.mod if args.command == "install" else [], args.command == "remove")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"LuaBuddy: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
