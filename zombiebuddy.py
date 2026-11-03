"""Download and inspect official ZombieBuddy JAR releases without installing them."""

import hashlib
from http.client import HTTPException
from io import BytesIO
import json
import re
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import Request, urlopen
import zipfile
import zlib


REPOSITORY = "https://github.com/zed-0xff/ZombieBuddy"
RELEASES_API = "https://api.github.com/repos/zed-0xff/ZombieBuddy/releases"
MAX_JSON_SIZE = 4 * 1024 * 1024
MAX_JAR_SIZE = 64 * 1024 * 1024
MAX_UNCOMPRESSED_SIZE = 256 * 1024 * 1024
MAX_MANIFEST_MAIN_SIZE = 64 * 1024
TIMEOUT = 30
MAX_RELEASE_PAGES = 10
VERSION = re.compile(r"v?(0|[1-9][0-9]{0,9})\.(0|[1-9][0-9]{0,9})\.(0|[1-9][0-9]{0,9})")


class ZombieBuddyError(ValueError):
    """An official release could not be retrieved or safely inspected."""


class ZombieBuddyNetworkError(ZombieBuddyError):
    """GitHub could not be reached or returned an HTTP error."""


def _version(tag: object) -> tuple[int, int, int] | None:
    match = VERSION.fullmatch(tag) if isinstance(tag, str) else None
    return tuple(map(int, match.groups())) if match else None


def validate_release(record: dict) -> dict:
    """Validate official JAR metadata and return only normalized release fields."""
    if not isinstance(record, dict):
        raise ZombieBuddyError("ZombieBuddy release metadata must be an object")
    tag = record.get("tag")
    if _version(tag) is None:
        raise ZombieBuddyError("ZombieBuddy release has an invalid stable version tag")
    for key in ("asset_id", "size"):
        if type(record.get(key)) is not int or record[key] <= 0:
            raise ZombieBuddyError(f"ZombieBuddy release has an invalid {key}")
    if record["size"] > MAX_JAR_SIZE:
        raise ZombieBuddyError("ZombieBuddy.jar exceeds the 64 MiB download limit")
    digest = record.get("sha256")
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise ZombieBuddyError("ZombieBuddy release has no valid GitHub SHA-256 digest")
    expected = {
        "url": f"{REPOSITORY}/releases/download/{tag}/ZombieBuddy.jar",
        "release_url": f"{REPOSITORY}/releases/tag/{tag}",
    }
    if any(record.get(key) != value for key, value in expected.items()):
        raise ZombieBuddyError("ZombieBuddy release does not point to the official GitHub JAR")
    result = {"tag": tag, "asset_id": record["asset_id"], "size": record["size"],
              "sha256": digest, **expected}
    if record.get("published_at") is not None:
        if not isinstance(record["published_at"], str):
            raise ZombieBuddyError("ZombieBuddy release has an invalid publication date")
        result["published_at"] = record["published_at"]
    return result


def _read_url(url: str, limit: int, label: str, accept: str) -> bytes:
    request = Request(url, headers={"User-Agent": "LuaBuddy-ZombieBuddy-updater",
                                    "Accept": accept, "X-GitHub-Api-Version": "2022-11-28"})
    try:
        with urlopen(request, timeout=TIMEOUT) as response:
            if response.getcode() != 200:
                raise ZombieBuddyNetworkError(f"GitHub returned HTTP {response.getcode()} for {label}")
            final_url = response.geturl()
            try:
                destination = urlsplit(final_url)
                port = destination.port
            except ValueError:
                raise ZombieBuddyError(f"GitHub redirected {label} to an invalid destination") from None
            allowed_hosts = {urlsplit(url).hostname}
            if accept != "application/vnd.github+json":
                allowed_hosts.update(("release-assets.githubusercontent.com", "objects.githubusercontent.com"))
            if (destination.scheme != "https" or destination.hostname not in allowed_hosts
                    or port not in (None, 443) or destination.username is not None or destination.password is not None):
                raise ZombieBuddyError(f"GitHub redirected {label} to an unexpected destination")
            if destination.hostname == urlsplit(url).hostname and final_url != url:
                raise ZombieBuddyError(f"GitHub redirected {label} outside its official release URL")
            length = response.headers.get("Content-Length")
            if length is not None:
                if not length.isascii() or not length.isdecimal():
                    raise ZombieBuddyError(f"GitHub returned an invalid size for {label}")
                if len(length) > 20 or int(length) > limit:
                    raise ZombieBuddyError(f"GitHub response for {label} exceeds its size limit")
            data = bytearray()
            while chunk := response.read(min(64 * 1024, limit - len(data) + 1)):
                data.extend(chunk)
                if len(data) > limit:
                    raise ZombieBuddyError(f"GitHub response for {label} exceeds its size limit")
            return bytes(data)
    except HTTPError as error:
        status = error.code
        error.close()
        hint = "; GitHub may be rate limiting requests, try again later" if status in (403, 429) else ""
        raise ZombieBuddyNetworkError(f"GitHub returned HTTP {status} for {label}{hint}") from None
    except (URLError, OSError, HTTPException):
        raise ZombieBuddyNetworkError(f"Could not retrieve {label} from GitHub; check your connection and try again") from None


def fetch_latest() -> dict:
    """Return validated metadata for the newest stable JAR, skipping installers."""
    page = 1
    candidates = []
    while True:
        body = _read_url(f"{RELEASES_API}?per_page=100&page={page}", MAX_JSON_SIZE,
                         "ZombieBuddy releases", "application/vnd.github+json")
        try:
            releases = json.loads(body)
        except (ValueError, UnicodeError, RecursionError):
            raise ZombieBuddyError("GitHub returned invalid ZombieBuddy release JSON") from None
        if not isinstance(releases, list):
            raise ZombieBuddyError("GitHub returned an invalid ZombieBuddy release list")
        for release in releases:
            if not isinstance(release, dict) or any(type(release.get(key)) is not bool for key in ("draft", "prerelease")):
                raise ZombieBuddyError("GitHub returned malformed ZombieBuddy release metadata")
            version = _version(release.get("tag_name"))
            if release["draft"] or release["prerelease"] or version is None:
                continue
            assets = release.get("assets")
            if not isinstance(assets, list) or any(not isinstance(asset, dict) for asset in assets):
                raise ZombieBuddyError("GitHub returned malformed ZombieBuddy asset metadata")
            jars = [asset for asset in assets if asset.get("name") == "ZombieBuddy.jar"]
            if len(jars) > 1:
                raise ZombieBuddyError("GitHub release contains more than one ZombieBuddy.jar")
            if jars:
                candidates.append((version, release, jars[0]))
        if len(releases) < 100:
            break
        if page >= MAX_RELEASE_PAGES:
            raise ZombieBuddyError("ZombieBuddy release history exceeds the updater pagination limit")
        page += 1
    if not candidates:
        raise ZombieBuddyError("No stable ZombieBuddy.jar release was found on official GitHub releases")
    _, release, asset = max(candidates, key=lambda candidate: candidate[0])
    digest = asset.get("digest")
    return validate_release({
        "tag": release["tag_name"], "asset_id": asset.get("id"), "size": asset.get("size"),
        "sha256": digest.removeprefix("sha256:")
        if isinstance(digest, str) and digest.startswith("sha256:") else None,
        "url": asset.get("browser_download_url"), "release_url": release.get("html_url"),
        "published_at": release.get("published_at"),
    })


def _manifest_main(stream) -> dict[str, str]:
    fields = []
    size = 0
    while True:
        line = stream.readline(MAX_MANIFEST_MAIN_SIZE + 1)
        size += len(line)
        if size > MAX_MANIFEST_MAIN_SIZE:
            raise ZombieBuddyError("ZombieBuddy JAR manifest main section exceeds its size limit")
        line = line.rstrip(b"\r\n")
        if not line:
            break
        if line.startswith(b" "):
            if not fields:
                raise ZombieBuddyError("ZombieBuddy JAR manifest has an invalid continuation")
            fields[-1] += line[1:]
        else:
            fields.append(line)
    result = {}
    for field in fields:
        key, separator, value = field.partition(b": ")
        if not separator or re.fullmatch(rb"[A-Za-z0-9][A-Za-z0-9_-]{0,69}", key) is None:
            raise ZombieBuddyError("ZombieBuddy JAR manifest has an invalid main attribute")
        name = key.decode("ascii").lower()
        if name in result:
            raise ZombieBuddyError("ZombieBuddy JAR manifest has duplicate main attributes")
        result[name] = value.decode("utf-8")
    return result


def inspect_jar(data: bytes) -> dict:
    """Check JAR integrity and return its version and manifest agent class."""
    if not data or len(data) > MAX_JAR_SIZE or not data.startswith(b"PK\x03\x04"):
        raise ZombieBuddyError("ZombieBuddy download is not a JAR within the 64 MiB limit")
    try:
        with BytesIO(data) as archive, zipfile.ZipFile(archive) as jar:
            entries = jar.infolist()
            names = {entry.filename for entry in entries}
            if len(names) != len(entries):
                raise ZombieBuddyError("ZombieBuddy JAR has duplicate ZIP entries")
            if sum(entry.file_size for entry in entries) > MAX_UNCOMPRESSED_SIZE:
                raise ZombieBuddyError("ZombieBuddy JAR exceeds the 256 MiB uncompressed limit")
            if any(entry.flag_bits & 1 or entry.compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)
                   for entry in entries):
                raise ZombieBuddyError("ZombieBuddy JAR uses unsupported ZIP encryption or compression")
            if jar.testzip() is not None:
                raise ZombieBuddyError("ZombieBuddy JAR failed its ZIP integrity check")
            if "META-INF/MANIFEST.MF" not in names:
                raise ZombieBuddyError("ZombieBuddy JAR has no manifest")
            with jar.open("META-INF/MANIFEST.MF") as stream:
                manifest = _manifest_main(stream)
            version = manifest.get("implementation-version")
            if _version(version) is None or version.startswith("v"):
                raise ZombieBuddyError("ZombieBuddy JAR manifest has no valid Implementation-Version")
            premain = manifest.get("premain-class", "")
            if (not premain or any(not part for part in premain.split("."))
                    or any(character.isspace() or character in "/\\;[\x00" for character in premain)):
                raise ZombieBuddyError("ZombieBuddy JAR manifest has no valid Premain-Class")
            class_file = premain.replace(".", "/") + ".class"
            if class_file not in names:
                raise ZombieBuddyError("ZombieBuddy JAR is missing its manifest agent class")
            with jar.open(class_file) as stream:
                if stream.read(4) != b"\xca\xfe\xba\xbe":
                    raise ZombieBuddyError("ZombieBuddy JAR agent entry is not a Java class")
            return {"version": version, "premain_class": premain}
    except (zipfile.BadZipFile, UnicodeError, RuntimeError, EOFError, zlib.error) as error:
        raise ZombieBuddyError("ZombieBuddy JAR has invalid ZIP or manifest data") from error


def download_release(release: dict) -> bytes:
    """Return a complete JAR only when it matches official metadata and its version."""
    metadata = validate_release(release)
    data = _read_url(metadata["url"], metadata["size"], "ZombieBuddy.jar", "application/octet-stream")
    if len(data) != metadata["size"]:
        raise ZombieBuddyError("ZombieBuddy.jar download size does not match the GitHub release")
    if hashlib.sha256(data).hexdigest() != metadata["sha256"]:
        raise ZombieBuddyError("ZombieBuddy.jar SHA-256 does not match the GitHub release")
    manifest = inspect_jar(data)
    if manifest["version"] != metadata["tag"].removeprefix("v"):
        raise ZombieBuddyError("ZombieBuddy.jar Implementation-Version does not match its release tag")
    return data
