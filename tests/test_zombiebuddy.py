import hashlib
from http.client import IncompleteRead
from io import BytesIO
import json
from pathlib import Path
import struct
import sys
import unittest
from unittest.mock import patch
from urllib.error import HTTPError, URLError
import warnings
import zipfile


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import zombiebuddy as buddy


REPOSITORY = "https://github.com/zed-0xff/ZombieBuddy"
CLASS = b"\xca\xfe\xba\xbe\x00\x00\x00\x45"


def jar_bytes(manifest=None, agent="example.Agent", class_data=CLASS, extra=(), compression=zipfile.ZIP_DEFLATED):
    if manifest is None:
        manifest = ("Manifest-Version: 1.0\r\nImplementation-Version: 2.3.4\r\n"
                    f"Premain-Class: {agent}\r\n\r\n").encode()
    stream = BytesIO()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", UserWarning)
        with zipfile.ZipFile(stream, "w", compression=compression) as jar:
            if manifest:
                jar.writestr("META-INF/MANIFEST.MF", manifest)
            if agent:
                jar.writestr(agent.replace(".", "/") + ".class", class_data)
            for name, data in extra:
                jar.writestr(name, data)
    return stream.getvalue()


def metadata(data, tag="v2.3.4"):
    return {"tag": tag, "asset_id": 607825569, "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
            "url": f"{REPOSITORY}/releases/download/{tag}/ZombieBuddy.jar",
            "release_url": f"{REPOSITORY}/releases/tag/{tag}", "published_at": "2026-10-03T00:00:00Z"}


def api_release(data, tag="v2.3.4", **changes):
    record = metadata(data, tag)
    return {"tag_name": tag, "draft": False, "prerelease": False,
            "html_url": record["release_url"], "published_at": record["published_at"],
            "assets": [{"id": record["asset_id"], "name": "ZombieBuddy.jar", "size": len(data),
                        "digest": "sha256:" + record["sha256"], "browser_download_url": record["url"]}], **changes}


class Response(BytesIO):
    def __init__(self, data, url=None, headers=None, status=200, read_error=None, chunk_size=None):
        super().__init__(json.dumps(data).encode() if not isinstance(data, bytes) else data)
        self.url = url
        self.headers = headers or {}
        self.status = status
        self.read_error = read_error
        self.chunk_size = chunk_size
        self.read_count = 0

    def getcode(self):
        return self.status

    def geturl(self):
        return self.url

    def read(self, size=-1):
        self.read_count += 1
        if self.read_error:
            raise self.read_error
        return super().read(min(size, self.chunk_size) if self.chunk_size else size)


class ZombieBuddyCheck(unittest.TestCase):
    def setUp(self):
        self.data = jar_bytes()

    def transport(self, responses):
        pending = iter(responses)

        def open_url(request, timeout):
            response = next(pending)
            if response.url is None:
                response.url = request.full_url
            return response

        return patch.object(buddy, "urlopen", side_effect=open_url)

    def test_release_discovery_and_download_use_the_official_jar(self):
        installer = api_release(self.data, "windows_installer_4.4", assets=[{"name": "ZombieBuddy.exe"}])
        matching_jar = jar_bytes(manifest=b"Implementation-Version: 2.10.1\nPremain-Class: example.Agent\n\n")
        newest = api_release(matching_jar, "v2.10.1")
        responses = [Response([installer, api_release(self.data, "v9.0.0", prerelease=True),
                               api_release(self.data, "v8.0.0", draft=True), api_release(self.data), newest]),
                     Response(matching_jar, "https://release-assets.githubusercontent.com/github-production-release-asset/file?sig=private",
                              chunk_size=7)]
        with self.transport(responses) as open_url:
            release = buddy.fetch_latest()
            self.assertEqual(release, metadata(matching_jar, "v2.10.1"))
            self.assertEqual(buddy.download_release(release), matching_jar)
            request = open_url.call_args.args[0]
            self.assertEqual(request.full_url, metadata(matching_jar, "v2.10.1")["url"])
            self.assertTrue(request.get_header("User-agent"))
            self.assertGreater(open_url.call_args.kwargs["timeout"], 0)
        self.assertTrue(all(response.closed for response in responses))

    def test_cached_metadata_is_normalized_without_persisting_unknown_fields(self):
        record = metadata(self.data)
        self.assertEqual(buddy.validate_release(dict(record, unknown="discarded")), record)
        record.pop("published_at")
        self.assertEqual(buddy.validate_release(record), record)
        unprefixed = metadata(self.data, "2.3.4")
        self.assertEqual(buddy.validate_release(unprefixed), unprefixed)

    def test_release_discovery_pages_past_installers(self):
        installer = api_release(self.data, "windows_installer_4.4", assets=[{"name": "ZombieBuddy.exe"}])
        responses = [Response([installer] * 100), Response([api_release(self.data)])]
        with self.transport(responses) as open_url:
            self.assertEqual(buddy.fetch_latest()["tag"], "v2.3.4")
            self.assertEqual([call.args[0].full_url for call in open_url.call_args_list],
                             [f"https://api.github.com/repos/zed-0xff/ZombieBuddy/releases?per_page=100&page={page}"
                              for page in (1, 2)])
        self.assertTrue(all(response.closed for response in responses))

    def test_newer_engine_release_can_appear_after_recent_backports(self):
        installer = api_release(self.data, "windows_installer_4.4", assets=[{"name": "ZombieBuddy.exe"}])
        responses = [Response([api_release(self.data, "v2.3.3")] + [installer] * 99), Response([api_release(self.data)])]
        with self.transport(responses):
            self.assertEqual(buddy.fetch_latest()["tag"], "v2.3.4")

    def test_release_pagination_is_bounded(self):
        installer = api_release(self.data, "windows_installer_4.4", assets=[{"name": "ZombieBuddy.exe"}])
        responses = [Response([installer] * 100) for _ in range(2)]
        with patch.object(buddy, "MAX_RELEASE_PAGES", 2), self.transport(responses):
            with self.assertRaises(buddy.ZombieBuddyError):
                buddy.fetch_latest()

    def test_discovery_fails_when_no_stable_exactly_named_jar_exists(self):
        for releases in ([], [api_release(self.data, "windows_installer_4.4")],
                         [api_release(self.data, "v3.0.0-rc.1")],
                         [api_release(self.data, assets=[{"name": "ZombieBuddy.jar.exe"}])]):
            with self.subTest(releases=releases), self.transport([Response(releases)]):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.fetch_latest()

    def test_malformed_api_responses_fail_and_close(self):
        cases = [b"not json", b"[" * 1500, {}, None, [None],
                 [api_release(self.data, prerelease="false")], [api_release(self.data, assets={})],
                 [api_release(self.data, assets=[None])]]
        duplicate = api_release(self.data)
        duplicate["assets"] *= 2
        cases.append([duplicate])
        for value in cases:
            response = Response(value)
            with self.subTest(value=str(value)[:60]), self.transport([response]):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.fetch_latest()
            self.assertTrue(response.closed)

    def test_unverifiable_newest_release_does_not_fall_back_to_an_older_jar(self):
        for digest in (None, "sha256:bad", "sha256:" + "A" * 64, "md5:" + "a" * 64):
            newest = api_release(self.data, "v3.0.0")
            newest["assets"][0]["digest"] = digest
            with self.subTest(digest=digest), self.transport([Response([api_release(self.data), newest])]):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.fetch_latest()

    def test_malformed_metadata_is_rejected_before_a_download(self):
        record = metadata(self.data)
        mutations = [(key, value) for key in ("asset_id", "size") for value in (True, 0, -1, "1")]
        mutations += [("size", buddy.MAX_JAR_SIZE + 1), ("tag", "windows_installer_4.4"),
                      ("sha256", "0" * 63), ("sha256", "A" * 64), ("published_at", 1)]
        mutations += [("url", url) for url in ("file:///ZombieBuddy.jar", "https://evil.example/ZombieBuddy.jar",
                      record["url"].replace("https:", "http:"), record["url"] + "?download=1",
                      record["url"].replace("zed-0xff", "someone-else"), record["url"] + ".exe")]
        mutations.append(("release_url", "https://github.com/another/repository/releases/tag/v2.3.4"))
        for key, value in mutations:
            with self.subTest(key=key, value=value), patch.object(buddy, "urlopen") as open_url:
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.download_release(dict(record, **{key: value}))
                open_url.assert_not_called()
        with self.assertRaises(buddy.ZombieBuddyError):
            buddy.download_release([])

    def test_download_rejects_digest_size_and_version_mismatches(self):
        cases = [(dict(metadata(self.data), sha256="0" * 64), self.data),
                 (dict(metadata(self.data), size=len(self.data) + 1), self.data),
                 (dict(metadata(self.data), size=len(self.data) - 1), self.data),
                 (metadata(self.data, "v3.0.0"), self.data)]
        for release, data in cases:
            response = Response(data)
            with self.subTest(release=release), self.transport([response]):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.download_release(release)
            self.assertTrue(response.closed)

    def test_a_github_asset_cdn_redirect_and_chunked_reads_are_supported(self):
        for host in ("release-assets.githubusercontent.com", "objects.githubusercontent.com"):
            response = Response(self.data, f"https://{host}/asset?sig=private", chunk_size=7)
            with self.subTest(host=host), self.transport([response]):
                self.assertEqual(buddy.download_release(metadata(self.data)), self.data)
            self.assertTrue(response.closed)

    def test_unexpected_redirects_are_rejected_without_exposing_their_urls(self):
        for destination in ("https://evil.example/asset?sig=SECRET", "http://objects.githubusercontent.com/SECRET",
                            "https://github.com/another/repository/SECRET", "https://objects.githubusercontent.com:bad/SECRET",
                            "https://name:SECRET@objects.githubusercontent.com/asset"):
            response = Response(self.data, destination)
            with self.subTest(destination=destination), self.transport([response]):
                with self.assertRaises(buddy.ZombieBuddyError) as caught:
                    buddy.download_release(metadata(self.data))
                self.assertNotIn("SECRET", str(caught.exception))
            self.assertTrue(response.closed)

    def test_json_and_download_response_limits_close_the_connection(self):
        for headers in ({"Content-Length": str(buddy.MAX_JSON_SIZE + 1)}, {"Content-Length": "-1"},
                        {"Content-Length": "invalid"}, {"Content-Length": "9" * 5000}):
            response = Response([], headers=headers)
            with self.subTest(headers=str(headers)[:50]), self.transport([response]):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.fetch_latest()
            self.assertTrue(response.closed)
            self.assertEqual(response.read_count, 0)
        response = Response(b" " * 65)
        with self.transport([response]), patch.object(buddy, "MAX_JSON_SIZE", 64):
            with self.assertRaises(buddy.ZombieBuddyError):
                buddy.fetch_latest()
        self.assertTrue(response.closed)
        response = Response(self.data, headers={"Content-Length": str(len(self.data) + 1)})
        with self.transport([response]):
            with self.assertRaises(buddy.ZombieBuddyError):
                buddy.download_release(metadata(self.data))
        self.assertTrue(response.closed)

    def test_transport_errors_have_context_and_close_error_responses(self):
        for error in (URLError("https://cdn.example/SECRET"), TimeoutError("SECRET")):
            with self.subTest(error=type(error).__name__), patch.object(buddy, "urlopen", side_effect=error):
                with self.assertRaises(buddy.ZombieBuddyError) as caught:
                    buddy.fetch_latest()
                self.assertIn("ZombieBuddy releases", str(caught.exception))
                self.assertNotIn("SECRET", str(caught.exception))
        stream = BytesIO(b"rate limited")
        error = HTTPError("https://cdn.example/SECRET", 403, "Forbidden", {}, stream)
        with patch.object(buddy, "urlopen", side_effect=error):
            with self.assertRaisesRegex(buddy.ZombieBuddyError, "rate limiting"):
                buddy.fetch_latest()
        self.assertTrue(stream.closed)
        for response in (Response(b"", read_error=IncompleteRead(b"short", 10)), Response(b"", status=503)):
            with self.transport([response]):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.fetch_latest()
            self.assertTrue(response.closed)

    def test_inspection_accepts_a_future_agent_class_and_only_reads_main_attributes(self):
        manifest = (b"Manifest-Version: 1.0\r\nImplementation-Version: 2.3.4\r\nPremain-Class: future.\r\n Agent\r\n\r\n"
                    + b"Name: a/class.class\r\nSHA-256-Digest: ignored\r\n\r\n" * 2000)
        data = jar_bytes(manifest=manifest, agent="future.Agent")
        self.assertEqual(buddy.inspect_jar(data), {"version": "2.3.4", "premain_class": "future.Agent"})

    def test_non_jars_and_corrupt_zip_data_are_rejected(self):
        data = jar_bytes(extra=[("payload.bin", b"CORRUPTION_SENTINEL")], compression=zipfile.ZIP_STORED)
        corrupted = data.replace(b"CORRUPTION_SENTINEL", b"CORRUPTION_SENTINE!", 1)
        for data in (b"", b"<html>GitHub error</html>", self.data[:20], corrupted):
            with self.subTest(data=data[:30]):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.inspect_jar(data)
        html = b"<html>Not a release</html>"
        with self.transport([Response(html)]):
            with self.assertRaises(buddy.ZombieBuddyError):
                buddy.download_release(metadata(html))

    def test_jar_limits_duplicate_entries_and_unsupported_compression_are_rejected(self):
        with patch.object(buddy, "MAX_JAR_SIZE", len(self.data) - 1):
            with self.assertRaises(buddy.ZombieBuddyError):
                buddy.inspect_jar(self.data)
        inflated = bytearray(self.data)
        central_entry = inflated.index(b"PK\x01\x02")
        struct.pack_into("<I", inflated, central_entry + 24, buddy.MAX_UNCOMPRESSED_SIZE + 1)
        for data in (bytes(inflated), jar_bytes(extra=[("example/Agent.class", CLASS)]),
                     jar_bytes(compression=zipfile.ZIP_BZIP2)):
            with self.subTest(size=len(data)):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.inspect_jar(data)

    def test_invalid_manifests_and_agent_entries_are_rejected(self):
        manifests = [b"", b"Premain-Class: example.Agent\n\n", b"Implementation-Version: 2.3.4\n\n",
                     b"Implementation-Version: 2.3.4\nPremain-Class: missing.Agent\n\n",
                     b"Implementation-Version: 2.3.4\nPremain-Class: ../example.Agent\n\n",
                     b"Implementation-Version: v2.3.4\nPremain-Class: example.Agent\n\n",
                     b"Implementation-Version: 2.3.4-beta\nPremain-Class: example.Agent\n\n",
                     b" invalid continuation\n\n", b"InvalidField\n\n", b"Bad@: value\n\n",
                     b"Implementation-Version: 2.3.4\nimplementation-version: 2.3.4\n\n",
                     b"Implementation-Version: \xff\n\n",
                     b"Long: " + b"x" * buddy.MAX_MANIFEST_MAIN_SIZE + b"\n\n"]
        for manifest in manifests:
            with self.subTest(manifest=manifest[:60]):
                with self.assertRaises(buddy.ZombieBuddyError):
                    buddy.inspect_jar(jar_bytes(manifest=manifest))
        with self.assertRaises(buddy.ZombieBuddyError):
            buddy.inspect_jar(jar_bytes(class_data=b"not a class"))


if __name__ == "__main__":
    unittest.main()
