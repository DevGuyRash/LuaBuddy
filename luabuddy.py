#!/usr/bin/env python3
"""Build and install LuaBuddy for Project Zomboid on Linux and Windows."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from typing import NamedTuple
from uuid import UUID

sys.dont_write_bytecode = True
import zombiebuddy


ROOT = Path(__file__).resolve().parent
AGENT = "-javaagent:.luabuddy/ZombieBuddy.jar=patches_jar=.luabuddy/LuaBuddy.jar:luabuddy"
CONFIG_PREFIX = "-Dluabuddy.config="
LEGACY_PREFIX = "-Dpzclientmods.config="
DEBUG_PROPERTY = "-Dluabuddy.debug"
REPORT_LIMIT = 4 * 1024 * 1024
METADATA_LIMIT = 64 * 1024
UPDATE_INTERVAL = 60 * 60


class Dependency(NamedTuple):
    release: dict
    data: bytes
    checked_at: str


class PreparedBuild(NamedTuple):
    helper: bytes
    record: dict
    dependency: Dependency


def game_directory(path: Path) -> Path | None:
    for candidate in (path, path / "projectzomboid"):
        if any((candidate / name).is_file() for name in ("projectzomboid.jar", "java/projectzomboid.jar")):
            return candidate.resolve()
    return None


def game_jars(game: Path) -> list[Path]:
    if (game / "projectzomboid.jar").is_file():
        return [game / "projectzomboid.jar"]
    return sorted((game / "java").glob("*.jar"))


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


def runtime_java(game: Path) -> Path:
    executable = "java.exe" if os.name == "nt" else "java"
    bundled = game / "jre64/bin" / executable
    return (bundled if bundled.is_file() else Path(jdk_tool("java"))).resolve()


def runtime_fingerprint(game: Path) -> str:
    java = runtime_java(game)
    digest = hashlib.sha256(str(java).encode("utf-8") + b"\0")
    for path in (java, java.parent.parent / "release"):
        digest.update(path.name.encode("utf-8") + b"\0")
        if path.is_file():
            digest.update(path.read_bytes())
        else:
            digest.update(b"<missing>")
        digest.update(b"\0")
    return digest.hexdigest()


def stage_file(path: Path, data: bytes, mode: int | None) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".luabuddy-", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(data)
        if mode is not None:
            temporary.chmod(mode)
        return temporary
    except OSError as error:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
        raise OSError(f"Preparing file {path}: {error}") from error


def apply_files(files: dict[Path, bytes]) -> bool:
    staged = {}
    applied = []
    try:
        for path, data in files.items():
            original = path.read_bytes() if path.exists() else None
            if original == data:
                continue
            mode = stat.S_IMODE(path.stat().st_mode) if original is not None else None
            temporary = stage_file(path, data, mode)
            staged[path] = [temporary, None]
            if original is not None:
                staged[path][1] = stage_file(path, original, mode)
        try:
            for path, (temporary, _) in staged.items():
                os.replace(temporary, path)
                applied.append(path)
        except OSError as error:
            failures = []
            for path in reversed(applied):
                try:
                    rollback = staged[path][1]
                    if rollback is None:
                        path.unlink()
                    else:
                        os.replace(rollback, path)
                except OSError:
                    failures.append(str(path))
            if failures:
                raise OSError(f"File update failed ({error}); rollback also failed for {', '.join(failures)}") from error
            raise
    finally:
        for pair in staged.values():
            for temporary in pair:
                if temporary is not None:
                    temporary.unlink(missing_ok=True)
    return bool(staged)


def atomic_write(path: Path, data: bytes) -> bool:
    return apply_files({path: data})


def json_bytes(value: dict) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def read_json(path: Path) -> dict:
    with path.open("rb") as stream:
        data = stream.read(METADATA_LIMIT + 1)
    if len(data) > METADATA_LIMIT:
        raise ValueError(f"Metadata exceeds {METADATA_LIMIT} bytes: {path}")
    try:
        value = json.loads(data)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise ValueError(f"Invalid metadata JSON: {path}") from error
    if not isinstance(value, dict):
        raise ValueError(f"Metadata must be an object: {path}")
    return value


def dependency_directory() -> Path:
    return ROOT / ".local/dependencies/zombiebuddy"


def verify_dependency(release: dict, data: bytes) -> None:
    if len(data) != release["size"] or hashlib.sha256(data).hexdigest() != release["sha256"]:
        raise ValueError("ZombieBuddy.jar does not match its verified release size and SHA-256")
    manifest = zombiebuddy.inspect_jar(data)
    if manifest["version"] != release["tag"].removeprefix("v"):
        raise ValueError("ZombieBuddy.jar version does not match its release metadata")


def jar_bytes(path: Path) -> bytes:
    with path.open("rb") as stream:
        data = stream.read(zombiebuddy.MAX_JAR_SIZE + 1)
    if len(data) > zombiebuddy.MAX_JAR_SIZE:
        raise ValueError(f"ZombieBuddy.jar exceeds its size limit: {path}")
    return data


def cached_release(release: dict) -> bytes:
    directory = dependency_directory() / str(release["asset_id"])
    stored = zombiebuddy.validate_release(read_json(directory / "release.json"))
    if stored != release:
        raise ValueError(f"Cached ZombieBuddy asset {release['asset_id']} metadata differs from the official release")
    data = jar_bytes(directory / "ZombieBuddy.jar")
    verify_dependency(release, data)
    return data


def cached_dependency() -> Dependency:
    path = dependency_directory() / "current.json"
    selection = read_json(path)
    if type(selection.get("schemaVersion")) is not int or selection["schemaVersion"] != 1:
        raise ValueError(f"Invalid dependency selection schema: {path}")
    release = zombiebuddy.validate_release(selection.get("release"))
    checked_at = selection.get("checkedAt")
    if not isinstance(checked_at, str) or re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", checked_at) is None:
        raise ValueError(f"Invalid dependency last-check date: {path}")
    datetime.strptime(checked_at, "%Y-%m-%dT%H:%M:%SZ")
    return Dependency(release, cached_release(release), checked_at)


def select_dependency(offline: bool = False, force: bool = False) -> Dependency:
    cached = None
    cache_error = None
    try:
        cached = cached_dependency()
    except (OSError, ValueError) as error:
        cache_error = error
    if offline:
        if cached is None:
            raise ValueError(f"No verified cached ZombieBuddy release available offline: {cache_error}")
        print(f"Offline: using verified cached ZombieBuddy {cached.release['tag']}; latest release not checked.")
        return cached
    now = datetime.now(timezone.utc)
    if cached is not None and not force:
        checked = datetime.strptime(cached.checked_at, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
        if 0 <= (now - checked).total_seconds() < UPDATE_INTERVAL:
            return cached
    try:
        release = zombiebuddy.validate_release(zombiebuddy.fetch_latest())
        directory = dependency_directory() / str(release["asset_id"])
        if (directory / "release.json").exists():
            stored = zombiebuddy.validate_release(read_json(directory / "release.json"))
            if stored != release:
                raise ValueError(f"ZombieBuddy asset {release['asset_id']} metadata changed; refusing to replace a cached release")
        try:
            data = cached_release(release)
        except (OSError, ValueError):
            data = zombiebuddy.download_release(release)
        verify_dependency(release, data)
    except zombiebuddy.ZombieBuddyNetworkError as error:
        if force or cached is None:
            raise
        print(f"Warning: {error}. Using verified cached ZombieBuddy {cached.release['tag']}; latest release not checked.", file=sys.stderr)
        return cached
    return Dependency(release, data, now.strftime("%Y-%m-%dT%H:%M:%SZ"))


def dependency_files(dependency: Dependency, select: bool = True) -> dict[Path, bytes]:
    directory = dependency_directory() / str(dependency.release["asset_id"])
    files = {directory / "ZombieBuddy.jar": dependency.data,
             directory / "release.json": json_bytes(dependency.release)}
    if select:
        files[dependency_directory() / "current.json"] = json_bytes({
            "schemaVersion": 1, "release": dependency.release, "checkedAt": dependency.checked_at})
    return files


def source_fingerprint() -> str:
    paths = [ROOT / "luabuddy.py", ROOT / "tests/EngineCheck.java"]
    paths.extend((ROOT / "src/luabuddy").glob("*.java"))
    paths.extend((ROOT / "resources").rglob("*"))
    paths.extend((ROOT / "profiles").glob("*"))
    digest = hashlib.sha256()
    for path in sorted(path for path in paths if path.is_file()):
        digest.update(path.relative_to(ROOT).as_posix().encode("utf-8") + b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def build_inputs(game: Path, dependency: Dependency) -> dict:
    digest = hashlib.sha256()
    for path in game_jars(game):
        digest.update(path.relative_to(game).as_posix().encode("utf-8") + b"\0")
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
    return {"schemaVersion": 1, "sources": source_fingerprint(),
            "game": digest.hexdigest(),
            "runtime": runtime_fingerprint(game),
            "framework": dependency.release["sha256"]}


def compile_and_verify(game: Path, dependency: Dependency) -> tuple[bytes, dict]:
    sources = sorted((ROOT / "src/luabuddy").glob("*.java"))
    if not sources or not (ROOT / "resources/bootstrap.lua").is_file() or not (ROOT / "tests/EngineCheck.java").is_file():
        raise ValueError("Missing Java sources, bootstrap resource, or native compatibility check")
    log_path = ROOT / ".local/compatibility.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=game, prefix=".luabuddy-check-") as directory, log_path.open("w", encoding="utf-8") as log:
        scratch = Path(directory)
        framework = scratch / "ZombieBuddy.jar"
        framework.write_bytes(dependency.data)
        helper = scratch / "LuaBuddy.jar"
        classes = scratch / "classes"
        checks = scratch / "checks"
        classes.mkdir()
        checks.mkdir()

        def relative(path: Path) -> str:
            return path.relative_to(game).as_posix()

        resource_directories = [game] + ([game / "java"] if (game / "java").is_dir() else [])
        classpath = os.pathsep.join([*map(relative, resource_directories), *map(relative, game_jars(game)), relative(framework)])

        def run(label: str, command: list[str], env=None) -> None:
            log.write(label + "\n")
            log.flush()
            try:
                subprocess.run(command, cwd=game, env=env, stdout=log, stderr=subprocess.STDOUT,
                               check=True, timeout=180)
            except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
                detail = "timed out" if isinstance(error, subprocess.TimeoutExpired) else "failed"
                raise ValueError(f"{label} {detail} for ZombieBuddy {dependency.release['tag']}; installed files unchanged. See {log_path}") from error

        run("Helper compilation", [jdk_tool("javac"), "--release", "25", "-proc:none", "-Xlint:all", "-Werror",
                                   "-cp", classpath, "-d", relative(classes), *map(str, sources)])
        run("Helper packaging", [jdk_tool("jar"), "--create", "--file", relative(helper),
                                 "-C", relative(classes), ".", "-C", str(ROOT / "resources"), "."])
        classpath = os.pathsep.join([classpath, relative(helper), relative(checks)])
        run("Native check compilation", [jdk_tool("javac"), "--release", "25", "-proc:none", "-Xlint:all", "-Werror",
                                         "-cp", classpath, "-d", relative(checks), str(ROOT / "tests/EngineCheck.java")])
        java = str(runtime_java(game))
        native_directories = [path for path in (game / "natives", game / "linux64", game / "win64", game) if path.is_dir()]
        env = os.environ.copy()
        env["LD_LIBRARY_PATH"] = os.pathsep.join([*map(str, native_directories), str(game / "jre64/lib"), env.get("LD_LIBRARY_PATH", "")])
        if os.name == "nt":
            env["PATH"] = os.pathsep.join([*map(str, native_directories), env.get("PATH", "")])
        agent = f"-javaagent:{relative(framework)}=patches_jar={relative(helper)}:luabuddy,config_dir={relative(scratch / 'agent-config')}"
        run("Native agent compatibility check", [java, "--enable-native-access=ALL-UNNAMED", "-Dzomboid.steam=0",
                                                "-Djava.library.path=" + os.pathsep.join(map(relative, native_directories)), "-Dluabuddy.verifyAgent=true", agent,
                                                "-cp", classpath, "luabuddy.EngineCheck", str(ROOT), str(scratch / "fixtures")], env)
        try:
            report = read_report(scratch / "fixtures/cache")
            if initialization_status(report) != "completed" or report["summary"]["loaded"] < 1:
                raise ValueError("Native agent did not complete the final fixture initialization")
            if report.get("zombieBuddyVersion") != dependency.release["tag"].removeprefix("v"):
                raise ValueError("Native report does not identify the candidate ZombieBuddy version")
        except (OSError, ValueError) as error:
            log.write(str(error) + "\n")
            raise ValueError(f"Native compatibility report failed validation; installed files unchanged. See {log_path}") from error
        return helper.read_bytes(), report


def prepare_build(game: Path, dependency: Dependency, reuse: bool = True) -> PreparedBuild:
    inputs = build_inputs(game, dependency)
    if reuse:
        try:
            helper = (ROOT / "dist/LuaBuddy.jar").read_bytes()
            record = read_json(ROOT / "dist/LuaBuddy.build.json")
            if (type(record.get("schemaVersion")) is int
                    and all(record.get(key) == value for key, value in inputs.items())
                    and record.get("verified") is True
                    and record.get("helper") == hashlib.sha256(helper).hexdigest()
                    and isinstance(record.get("gameVersion"), str) and record["gameVersion"]
                    and record.get("zombieBuddyVersion") == dependency.release["tag"].removeprefix("v")):
                return PreparedBuild(helper, record, dependency)
        except (OSError, ValueError):
            pass
    helper, report = compile_and_verify(game, dependency)
    record = dict(inputs, helper=hashlib.sha256(helper).hexdigest(), verified=True,
                  gameVersion=report["gameVersion"], zombieBuddyVersion=report["zombieBuddyVersion"])
    return PreparedBuild(helper, record, dependency)


def build_files(prepared: PreparedBuild) -> dict[Path, bytes]:
    return {ROOT / "dist/LuaBuddy.jar": prepared.helper,
            ROOT / "dist/LuaBuddy.build.json": json_bytes(prepared.record),
            **dependency_files(prepared.dependency)}


def build(game: Path, offline: bool = False) -> None:
    prepared = prepare_build(game, select_dependency(offline=offline))
    apply_files(build_files(prepared))
    print(f"Built {ROOT / 'dist/LuaBuddy.jar'}; verified with ZombieBuddy {prepared.dependency.release['tag']} and Project Zomboid {prepared.record['gameVersion']}.")


def compatibility(game: Path, offline: bool = False) -> None:
    prepared = prepare_build(game, select_dependency(offline=offline), reuse=False)
    apply_files(dependency_files(prepared.dependency, select=False))
    print(f"Compatibility passed for Project Zomboid {prepared.record['gameVersion']} with ZombieBuddy {prepared.dependency.release['tag']}. Native APIs and the instrumented OnGameStart event passed.")


def check_update(game: Path) -> int:
    release = zombiebuddy.validate_release(zombiebuddy.fetch_latest())
    installed = game / ".luabuddy/ZombieBuddy.jar"
    if not installed.is_file():
        print(f"ZombieBuddy is not installed. Latest stable JAR: {release['tag']} ({release['sha256']}).")
        return 2
    data = jar_bytes(installed)
    manifest = zombiebuddy.inspect_jar(data)
    digest = hashlib.sha256(data).hexdigest()
    print(f"Installed ZombieBuddy {manifest['version']}, SHA-256 {digest}.")
    print(f"Latest stable JAR {release['tag']}, SHA-256 {release['sha256']}; {release['release_url']}")
    if digest != release["sha256"] or len(data) != release["size"] or manifest["version"] != release["tag"].removeprefix("v"):
        print("Update available. Run luabuddy.py update, then fully restart Project Zomboid.")
        return 2
    print("Installed ZombieBuddy is current. Running sessions require a full restart to load installed changes.")
    return 0


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
    for mod_id in ids:
        if not mod_id.strip() or any(ord(char) < 32 or ord(char) == 127 for char in mod_id):
            raise ValueError("Mod IDs must be nonempty and contain no control characters")
    # Java's String.trim() removes ASCII padding; controls remain invalid input.
    ids = [mod_id.strip(" ") for mod_id in ids]
    files = {}
    names = {}
    existing = {}
    identities = {}
    if directory.exists():
        for path in directory.iterdir():
            existing.setdefault(path.name.casefold(), []).append(path)
            if ids and path.is_file() and path.suffix.lower() == ".properties":
                text = path.read_text(encoding="utf-8")
                entries = re.findall(r"(?m)^id=(.*)$", text)
                id_keys = re.findall(r"(?m)^[ \t\f]*id(?:[ \t\f]*[=:]|[ \t\f]+)", text)
                conventional = len(entries) == 1 and len(id_keys) == 1
                for line in text.splitlines():
                    stripped = line.lstrip()
                    if not stripped or stripped.startswith(("#", "!")):
                        continue
                    key = re.split(r"[ \t\f=:]", stripped, maxsplit=1)[0]
                    if "\\" in key or (len(line) - len(line.rstrip("\\"))) % 2:
                        conventional = False
                if not conventional or not re.fullmatch(r"(?:[^\\\x00-\x1f\x7f]|\\[\\ ])*", entries[0]):
                    raise ValueError(f"Cannot safely read mod ID in {path}; use one unindented id= entry with plain text, escaped spaces or escaped backslashes before using --mod; no files changed")
                identities[path] = re.sub(r"\\([\\ ])", r"\1", entries[0]).strip(" ")
    for mod_id in dict.fromkeys(ids):
        reserved = re.fullmatch(r"CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9]", mod_id.split(".", 1)[0], re.IGNORECASE)
        portable = re.fullmatch(r"[A-Za-z0-9_.-]+", mod_id) and not reserved and not mod_id.endswith(".") and len(mod_id) <= 180
        stem = mod_id if portable else "mod-" + hashlib.sha256(mod_id.encode("utf-8")).hexdigest()[:16]
        source = ROOT / "profiles" / (stem + ".properties")
        target = directory / source.name
        key = target.name.casefold()
        if key in names:
            raise ValueError(f"Mod IDs {names[key]!r} and {mod_id!r} collide at profile filename {target.name!r}; no files changed")
        names[key] = mod_id
        matches = existing.get(key, [])
        if matches and (len(matches) != 1 or matches[0].name != target.name):
            raise ValueError(f"Profile filename {target.name!r} collides with an existing filename; no files changed")
        escaped_id = mod_id.replace("\\", "\\\\").replace(" ", "\\ ")
        if any(path != target and mod_id == identity for path, identity in identities.items()):
            raise ValueError(f"Mod ID {mod_id!r} already appears under another profile filename; resolve the ambiguous profile before installing; no files changed")
        if matches:
            if identities.get(target) != mod_id:
                raise ValueError(f"Cannot verify that existing profile {target} belongs to mod ID {mod_id!r}; expected one plain id= entry; no files changed")
        if not target.exists():
            files[target] = source.read_bytes() if source.is_file() else f"id={escaped_id}\n".encode("utf-8")
        plugin = ROOT / "profiles" / (stem + ".lua")
        if source.is_file() and plugin.is_file():
            matches = existing.get(plugin.name.casefold(), [])
            if matches and (len(matches) != 1 or matches[0].name != plugin.name or not matches[0].is_file()):
                raise ValueError(f"Plugin filename {plugin.name!r} collides with an existing filename; no files changed")
            if not matches:
                files[directory / plugin.name] = plugin.read_bytes()
    return files


def debug_argument(argument: str) -> bool:
    return argument == DEBUG_PROPERTY or argument.startswith(DEBUG_PROPERTY + "=")


def debug_value(args: list[str]) -> str | None:
    values = [arg.partition("=")[2] for arg in args if debug_argument(arg)]
    return values[-1] if values else None


def configure(game: Path, launcher: Path, directory: Path, ids: list[str], remove: bool,
              debug: bool | None = None, offline: bool = False, force: bool = False) -> None:
    original, config, args = launcher_data(launcher)
    if remove and any(buddy_agent(arg) and "LuaBuddy.jar:luabuddy" in arg and not owned_agent(arg) for arg in args):
        raise ValueError("LuaBuddy shares a ZombieBuddy agent with other options. Remove its patch manually; launcher unchanged.")
    updated = [arg for arg in args if not arg.startswith(CONFIG_PREFIX) and not owned_agent(arg)]
    pending = {}
    if remove or debug is not None:
        updated = [arg for arg in updated if not debug_argument(arg)]
    if not remove:
        updated = [arg for arg in updated if not arg.startswith(LEGACY_PREFIX) and not owned_agent(arg, legacy=True)]
        if any(buddy_agent(arg) for arg in updated):
            raise ValueError("Another ZombieBuddy agent or patch configuration is present. Combine it manually before installing LuaBuddy; launcher unchanged.")
        pending = profile_files(ids, directory)
        config_path = directory.as_posix()
        if "\n" in config_path or "\r" in config_path:
            raise ValueError("The profile directory must not contain a line break")
        if debug:
            updated.append(DEBUG_PROPERTY + "=true")
        updated.extend([CONFIG_PREFIX + config_path, AGENT])
        prepared = prepare_build(game, select_dependency(offline=offline, force=force))
        pending.update(build_files(prepared))
        pending.update({game / ".luabuddy/LuaBuddy.jar": prepared.helper,
                        game / ".luabuddy/ZombieBuddy.jar": prepared.dependency.data,
                        game / ".luabuddy/ZombieBuddy.release.json": json_bytes(prepared.dependency.release)})
    changed = updated != args
    backup = ROOT / ".local/backups" / (hashlib.sha256(str(launcher).encode()).hexdigest()[:16] + "-" + launcher.name + ".original")
    if changed:
        if not backup.exists():
            pending[backup] = original
        config["vmArgs"] = updated
    if changed:
        pending[launcher] = (json.dumps(config, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    if not remove:
        directory.mkdir(parents=True, exist_ok=True)
    apply_files(pending)
    print(("Updated " if changed else "Already configured ") + str(launcher))
    if changed:
        print(f"Original launcher backup: {backup}")
    if remove:
        print(f"Runtime files retained at {game / '.luabuddy'}; profiles retained at {directory}")
    else:
        print(f"Profiles: {directory}")
        print(f"Installed ZombieBuddy {prepared.dependency.release['tag']}; helper verified for Project Zomboid {prepared.record['gameVersion']}.")
    print("Fully restart Project Zomboid to load installed changes on the next normal Steam launch. Running sessions are unaffected.")


def status(game: Path, launcher: Path) -> None:
    _, _, args = launcher_data(launcher)
    enabled = any(owned_agent(arg) for arg in args)
    print(f"LuaBuddy {'enabled' if enabled else 'not enabled'} in {launcher}")
    for arg in args:
        if arg.startswith(CONFIG_PREFIX) or owned_agent(arg):
            print(arg)
    value = debug_value(args)
    print(f"Debug property: {value!r}" if value is not None else "Debug property: unset (false)")
    for name in ("LuaBuddy.jar", "ZombieBuddy.jar"):
        path = game / ".luabuddy" / name
        print(f"Runtime {'present' if path.is_file() else 'missing'}: {path}")


def read_report(cache: Path) -> dict:
    path = cache / "LuaBuddy/diagnostics.json"
    with path.open("rb") as stream:
        data = stream.read(REPORT_LIMIT + 1)
    if len(data) > REPORT_LIMIT:
        raise ValueError(f"Loader initialization report exceeds {REPORT_LIMIT} bytes: {path}")
    try:
        report = json.loads(data)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise ValueError(f"Invalid loader initialization report JSON: {path}") from error

    def require(condition: bool, field: str) -> None:
        if not condition:
            raise ValueError(f"Invalid loader initialization report field {field}: {path}")

    require(isinstance(report, dict), "root")
    require(type(report.get("schemaVersion")) is int and report["schemaVersion"] == 1, "schemaVersion")
    require(report.get("kind") == "loader-initialization", "kind")
    result = {"schemaVersion": 1, "kind": "loader-initialization"}
    for field in ("version", "session", "startedAt", "completedAt", "gameVersion", "javaVersion", "configDirectory"):
        require(isinstance(report.get(field), str), field)
        result[field] = report[field]
    try:
        UUID(result["session"])
        for field in ("startedAt", "completedAt"):
            value = result[field]
            require(re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|\+00:00)", value) is not None, field)
            datetime.strptime(value[:19], "%Y-%m-%dT%H:%M:%S")
    except ValueError as error:
        raise ValueError(f"Invalid loader initialization report session or UTC timestamp: {path}") from error
    duration = report.get("durationMs")
    require(type(duration) in (int, float) and duration >= 0 and (type(duration) is int or math.isfinite(duration)), "durationMs")
    require(type(report.get("debug")) is bool, "debug")
    result.update(durationMs=duration, debug=report["debug"])
    for field in ("error", "traceback", "zombieBuddyVersion"):
        if field in report:
            require(isinstance(report[field], str), field)
            result[field] = report[field]
    summary = report.get("summary")
    require(isinstance(summary, dict), "summary")
    result["summary"] = {}
    for field in ("loaded", "serverLoaded", "failed", "disabled", "pending"):
        require(type(summary.get(field)) is int and summary[field] >= 0, "summary." + field)
        result["summary"][field] = summary[field]
    mods = report.get("mods")
    require(isinstance(mods, dict), "mods")
    result["mods"] = {}
    for mod_id, record in mods.items():
        require(isinstance(record, dict), "mods record")
        require(record.get("status") in ("loaded", "server-loaded", "disabled", "failed", "prepared"), "mods.status")
        require(isinstance(record.get("stage"), str), "mods.stage")
        entry = {"status": record["status"], "stage": record["stage"]}
        for field in ("profile", "commonRoot", "versionRoot", "requires", "preload", "plugin", "source", "error", "traceback", "javaTraceback"):
            if field in record:
                require(isinstance(record[field], str), "mods." + field)
                entry[field] = record[field]
        if "sourceCount" in record:
            require(type(record["sourceCount"]) is int and record["sourceCount"] >= 0, "mods.sourceCount")
            entry["sourceCount"] = record["sourceCount"]
        result["mods"][mod_id] = entry
    return result


def initialization_status(report: dict) -> str:
    if report.get("error") or report["summary"]["failed"] or any(mod["status"] == "failed" for mod in report["mods"].values()):
        return "failed"
    if report["summary"]["pending"] or any(mod["status"] == "prepared" for mod in report["mods"].values()):
        return "incomplete"
    return "completed"


def diagnostics(cache: Path, as_json: bool) -> int:
    report = read_report(cache)
    state = initialization_status(report)
    if as_json:
        print(json.dumps(report, indent=2))
    else:
        print(f"Last loader initialization: {state}")
        print(f"Session: {report['session']}")
        print(f"Started: {report['startedAt']}; completed: {report['completedAt']}; duration: {report['durationMs']} ms")
        if "zombieBuddyVersion" in report:
            print(f"ZombieBuddy at initialization: {report['zombieBuddyVersion']}")
        print("This report covers loader initialization only; later callbacks are not checked.")
        print("Report paths may identify your local user account or directories.")
        if report.get("error"):
            print("Error: " + report["error"])
        if report.get("traceback"):
            print("Traceback: " + report["traceback"])
        for mod_id, record in report["mods"].items():
            print(f"{mod_id}: {record['status']} at {record['stage']}")
            for field in ("source", "plugin", "error", "traceback", "javaTraceback"):
                if record.get(field):
                    print(f"  {field}: {record[field]}")
    return 1 if state == "failed" else 0


def doctor(game: Path, launcher: Path, cache: Path, as_json: bool) -> int:
    checks = []

    def check(name: str, state: str, message: str) -> None:
        checks.append({"name": name, "status": state, "message": message})

    directory = None
    value = None
    try:
        _, _, args = launcher_data(launcher)
        agents = [arg for arg in args if owned_agent(arg)]
        check("launcher-agent", "ok" if agents == [AGENT] else "error",
              "Selected launcher has the installed LuaBuddy agent." if agents == [AGENT]
              else "Selected launcher must have exactly one LuaBuddy agent using the installed .luabuddy runtime paths.")
        if any(buddy_agent(arg) and not owned_agent(arg) for arg in args):
            check("launcher-conflict", "error", "Selected launcher also has another ZombieBuddy agent configuration.")
        configs = [arg[len(CONFIG_PREFIX):] for arg in args if arg.startswith(CONFIG_PREFIX)]
        if len(configs) == 1 and configs[0]:
            directory = Path(configs[0])
            if not directory.is_absolute():
                directory = game / directory
            directory = directory.resolve()
            check("launcher-config", "ok", f"Profile directory: {directory}")
        else:
            check("launcher-config", "error", "Selected launcher must have exactly one nonempty luabuddy.config property.")
        value = debug_value(args)
        check("launcher-debug", "ok", f"Debug property: {value!r}" if value is not None else "Debug property: unset (false)")
    except (OSError, ValueError) as error:
        check("launcher", "error", str(error))
    for name, reference in (("LuaBuddy.jar", ROOT / "dist/LuaBuddy.jar"),):
        installed = game / ".luabuddy" / name
        try:
            if not installed.is_file():
                check(name, "error", f"Runtime missing: {installed}")
            elif not reference.is_file():
                check(name, "warning", f"Runtime present; repository artifact unavailable for comparison: {reference}")
            elif hashlib.sha256(installed.read_bytes()).digest() != hashlib.sha256(reference.read_bytes()).digest():
                check(name, "error", f"Installed runtime differs from repository artifact: {installed}")
            else:
                check(name, "ok", f"Runtime matches repository artifact: {installed}")
        except OSError as error:
            check(name, "error", str(error))
    installed = game / ".luabuddy/ZombieBuddy.jar"
    try:
        data = jar_bytes(installed)
        manifest = zombiebuddy.inspect_jar(data)
        digest = hashlib.sha256(data).hexdigest()
        installed_label = f"Installed ZombieBuddy {manifest['version']}, SHA-256 {digest}."
        metadata_path = game / ".luabuddy/ZombieBuddy.release.json"
        if metadata_path.is_file():
            release = zombiebuddy.validate_release(read_json(metadata_path))
            verify_dependency(release, data)
            installed_label = f"Installed ZombieBuddy {release['tag']}, SHA-256 {digest}."
        try:
            dependency = cached_dependency()
        except (OSError, ValueError) as error:
            check("ZombieBuddy.jar", "warning", installed_label + f" Verified dependency cache unavailable: {error}")
        else:
            match = digest == dependency.release["sha256"]
            check("ZombieBuddy.jar", "ok" if match else "warning", installed_label
                  + (" Matches" if match else " Differs from")
                  + f" verified cached {dependency.release['tag']}; last checked {dependency.checked_at}.")
    except (OSError, ValueError) as error:
        check("ZombieBuddy.jar", "error", str(error))
    if directory is not None:
        if not directory.is_dir():
            check("profiles", "error", f"Profile directory missing: {directory}")
        elif not any(path.is_file() for path in directory.glob("*.properties")):
            check("profiles", "warning", f"No .properties profiles in {directory}")
        else:
            check("profiles", "ok", "Profile directory contains .properties profiles.")
    metadata = None
    try:
        report = read_report(cache)
        metadata = {key: report[key] for key in ("schemaVersion", "kind", "version", "session", "startedAt", "completedAt", "durationMs", "gameVersion", "javaVersion", "configDirectory", "debug", "summary")}
        if "zombieBuddyVersion" in report:
            metadata["zombieBuddyVersion"] = report["zombieBuddyVersion"]
        state = initialization_status(report)
        framework_version = f" ZombieBuddy {report['zombieBuddyVersion']}." if "zombieBuddyVersion" in report else ""
        check("last-initialization", "error" if state == "failed" else "warning" if state == "incomplete" else "ok",
              f"Last loader initialization {state}, session {report['session']}, completed {report['completedAt']}." + framework_version
              + " Later callbacks are not checked. Fully restart Project Zomboid after installing changes.")
        if directory is None or Path(report["configDirectory"]) != directory or report["debug"] != (value is not None and value.lower() == "true"):
            check("report-configuration", "warning", "Last loader initialization used different settings; the selected launcher settings are not runtime verified.")
    except FileNotFoundError:
        check("last-initialization", "warning", "No loader initialization report; runtime behavior has not been verified.")
    except (OSError, ValueError) as error:
        check("last-initialization", "error", str(error))
    result = "error" if any(item["status"] == "error" for item in checks) else "warning" if any(item["status"] == "warning" for item in checks) else "ok"
    if as_json:
        print(json.dumps({"result": result, "checks": checks, "report": metadata}, indent=2))
    else:
        print(f"Doctor result: {result}. Checks cover configuration and the last loader initialization only.")
        for item in checks:
            print(f"{item['status'].upper()} {item['name']}: {item['message']}")
    return 1 if result == "error" else 0


def main() -> int:
    sys.stdout.reconfigure(errors="backslashreplace")
    sys.stderr.reconfigure(errors="backslashreplace")
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    for name in ("build", "install", "update", "compatibility", "remove", "status", "doctor", "diagnostics"):
        command = subparsers.add_parser(name)
        if name in ("doctor", "diagnostics"):
            command.add_argument("--cache-dir", type=Path, default=Path.home() / "Zomboid")
            command.add_argument("--json", action="store_true")
        if name == "diagnostics":
            continue
        command.add_argument("--game-dir", type=Path)
        command.add_argument("--launcher", type=Path)
        if name in ("install", "remove"):
            command.add_argument("--config-dir", type=Path, default=ROOT / "config")
        if name in ("build", "install", "compatibility"):
            command.add_argument("--offline", action="store_true", help="Use a verified cached framework without checking GitHub")
        if name == "update":
            command.add_argument("--check", action="store_true", help="Check GitHub without changing files; exit 2 when an update is available")
        if name == "install":
            command.add_argument("--mod", action="append", default=[])
            command.add_argument("--debug", action=argparse.BooleanOptionalAction, default=None)
    args = parser.parse_args()
    try:
        if args.command == "diagnostics":
            return diagnostics(args.cache_dir.expanduser(), args.json)
        game = find_game(args.game_dir or (args.launcher.parent if args.launcher else None))
        launcher = args.launcher.expanduser().resolve() if args.launcher else game / "ProjectZomboid64.json"
        if launcher.parent != game:
            raise ValueError(f"Launcher must be in the selected game directory {game}; relative runtime paths use that game's working directory")
        if args.command == "build":
            build(game, args.offline)
        elif args.command == "compatibility":
            compatibility(game, args.offline)
        elif args.command == "update":
            if args.check:
                return check_update(game)
            _, _, arguments = launcher_data(launcher)
            configs = [argument[len(CONFIG_PREFIX):] for argument in arguments if argument.startswith(CONFIG_PREFIX)]
            if len(configs) > 1 or (configs and not configs[0]):
                raise ValueError("Cannot preserve ambiguous launcher profile settings; no files changed")
            directory = Path(configs[0]) if configs else ROOT / "config"
            if not directory.is_absolute():
                directory = game / directory
            configure(game, launcher, directory.expanduser().resolve(), [], False, force=True)
        elif args.command == "status":
            status(game, launcher)
        elif args.command == "doctor":
            return doctor(game, launcher, args.cache_dir.expanduser(), args.json)
        else:
            configure(game, launcher, args.config_dir.expanduser().resolve(),
                      args.mod if args.command == "install" else [], args.command == "remove",
                      args.debug if args.command == "install" else None,
                      args.offline if args.command == "install" else False)
    except (OSError, ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"LuaBuddy: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
