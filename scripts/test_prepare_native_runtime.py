"""Stdlib-only, terminal tests; temporary files stay in the deps directory's tmp folder."""

import importlib.util
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile


SCRIPT = Path(__file__).with_name("prepare-native-runtime.py")
SPEC = importlib.util.spec_from_file_location("prepare_native_runtime", SCRIPT)
runtime = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = runtime
SPEC.loader.exec_module(runtime)


class PrepareRuntimeTests(unittest.TestCase):
    def setUp(self):
        temporary = runtime.DEPS / "tmp"
        temporary.mkdir(parents=True, exist_ok=True)
        self.context = tempfile.TemporaryDirectory(dir=temporary, prefix="test-native-setup-")
        self.addCleanup(self.context.cleanup)
        self.deps = Path(self.context.name)
        (self.deps / "downloads").mkdir(parents=True)

    def archive(self, name, entries, kind="tar"):
        path = self.deps / "downloads" / name
        if kind == "zip":
            with zipfile.ZipFile(path, "w") as output:
                for filename, data in entries:
                    member = zipfile.ZipInfo(filename)
                    # ZipInfo otherwise normalizes backslashes on Windows,
                    # hiding the hostile cross-platform path from the test.
                    member.filename = filename
                    output.writestr(member, data)
        else:
            with tarfile.open(path, "w:bz2") as output:
                for filename, data in entries:
                    member = tarfile.TarInfo(filename)
                    if isinstance(data, tuple):
                        member.type, member.linkname = data
                        output.addfile(member)
                    else:
                        member.size = len(data)
                        output.addfile(member, io.BytesIO(data))
        artifact = runtime.Artifact(name, path.stat().st_size, runtime.sha256(path), "https://example.invalid/" + name)
        return path, artifact

    def windows_archives(self):
        sherpa = self.archive("sherpa.tar.bz2", [
            ("sherpa/lib/sherpa-onnx-c-api.dll", b"sherpa"),
            ("sherpa/lib/sherpa-onnx-c-api.lib", b"sherpa-import"),
            ("sherpa/lib/onnxruntime.dll", b"old-runtime"),
            ("sherpa/lib/onnxruntime.lib", b"old-import"),
            ("sherpa/lib/onnxruntime_providers_shared.dll", b"old-provider"),
        ])
        ort = self.archive("ort.zip", [
            ("ort/lib/onnxruntime.dll", b"new-runtime"),
            ("ort/lib/onnxruntime.lib", b"new-import"),
            ("ort/lib/onnxruntime_providers_shared.dll", b"new-provider"),
            ("ort/lib/onnxruntime.pdb", b"ignored-debug-symbols"),
        ], kind="zip")
        return sherpa, ort

    def test_prepare_reuses_cache_replaces_complete_runtime_and_is_idempotent(self):
        sherpa, ort = self.windows_archives()
        output = self.deps / "native/test"
        with patch.dict(runtime.ARTIFACTS, {"windows-x64": (sherpa[1], ort[1])}), patch.object(runtime.urllib.request, "urlopen", side_effect=AssertionError("Network was accessed")):
            first = runtime.prepare("windows-x64", output, True, self.deps)
            second = runtime.prepare("windows-x64", output, True, self.deps)
        self.assertEqual(first, second)
        self.assertEqual((output / "lib/onnxruntime.dll").read_bytes(), b"new-runtime")
        self.assertEqual((output / "lib/onnxruntime_providers_shared.dll").read_bytes(), b"new-provider")
        self.assertFalse((output / "lib/onnxruntime.pdb").exists())
        self.assertFalse(list((self.deps / "downloads").glob("*.part")))
        self.assertEqual(json.loads((output / runtime.MANIFEST).read_text())["archives"][1]["sha256"], ort[1].sha256)

    def test_modified_runtime_and_unmanaged_directory_are_not_overwritten(self):
        sherpa, ort = self.windows_archives()
        output = self.deps / "native/test"
        with patch.dict(runtime.ARTIFACTS, {"windows-x64": (sherpa[1], ort[1])}):
            runtime.prepare("windows-x64", output, True, self.deps)
            (output / "lib/onnxruntime.dll").write_bytes(b"user-modified")
            with self.assertRaisesRegex(ValueError, "modified or incomplete"):
                runtime.prepare("windows-x64", output, True, self.deps)
            (output / runtime.MANIFEST).unlink()
            with self.assertRaisesRegex(ValueError, "unmanaged"):
                runtime.prepare("windows-x64", output, True, self.deps)
            with self.assertRaisesRegex(ValueError, "do not match"):
                runtime.prepare("windows-x64", output, True, self.deps, adopt_existing=True)
        self.assertEqual((output / "lib/onnxruntime.dll").read_bytes(), b"user-modified")

    def test_legacy_tts_manifest_and_adoption_cannot_relabel_or_overwrite_binaries(self):
        legacy_sherpa, ort = self.windows_archives()
        no_tts = self.archive("sherpa-no-tts.tar.bz2", [
            ("sherpa-no-tts/lib/sherpa-onnx-c-api.dll", b"asr-no-tts"),
            ("sherpa-no-tts/lib/sherpa-onnx-c-api.lib", b"asr-no-tts-import"),
        ])
        legacy_output = self.deps / "native/sherpa-legacy"
        new_output = self.deps / runtime.default_output("windows-x64")
        with patch.dict(runtime.ARTIFACTS, {"windows-x64": (legacy_sherpa[1], ort[1])}):
            runtime.prepare("windows-x64", legacy_output, True, self.deps)
        manifest_path = legacy_output / runtime.MANIFEST
        legacy_manifest = json.loads(manifest_path.read_text())
        legacy_manifest["schema"] = 1
        legacy_manifest.pop("sherpa_build")
        manifest_path.write_text(json.dumps(legacy_manifest))
        saved_manifest = manifest_path.read_bytes()
        with patch.dict(runtime.ARTIFACTS, {"windows-x64": (no_tts[1], ort[1])}), patch.object(runtime.urllib.request, "urlopen", side_effect=AssertionError("Network was accessed")):
            with self.assertRaisesRegex(ValueError, "legacy pinned artifacts"):
                runtime.prepare("windows-x64", legacy_output, True, self.deps)
            self.assertEqual(manifest_path.read_bytes(), saved_manifest)
            self.assertEqual((legacy_output / "lib/sherpa-onnx-c-api.dll").read_bytes(), b"sherpa")
            # Even explicit adoption must compare actual file hashes, rather
            # than adding a no-tts feature label to a legacy binary directory.
            manifest_path.unlink()
            with self.assertRaisesRegex(ValueError, "do not match"):
                runtime.prepare("windows-x64", legacy_output, True, self.deps, adopt_existing=True)
            prepared = runtime.prepare("windows-x64", new_output, True, self.deps)
        self.assertEqual(prepared["schema"], 2)
        self.assertEqual(prepared["sherpa_build"], "no-tts")
        self.assertEqual((new_output / "lib/sherpa-onnx-c-api.dll").read_bytes(), b"asr-no-tts")
        self.assertEqual((new_output / "lib/onnxruntime.dll").read_bytes(), b"new-runtime")
        self.assertEqual((legacy_output / "lib/sherpa-onnx-c-api.dll").read_bytes(), b"sherpa")

    def test_default_no_tts_paths_keep_both_legacy_platform_runtimes_separate(self):
        for target, prefix in [("windows-x64", ""), ("linux-x64", "linux-x64-")]:
            with self.subTest(target=target):
                output = runtime.default_output(target)
                legacy = Path(f"native/{prefix}sherpa-{runtime.SHERPA_VERSION}-ort-{runtime.ORT_VERSION}")
                self.assertNotEqual(output, legacy)
                self.assertTrue(output.is_relative_to(Path("native")))
                self.assertIn("no-tts", output.name)

    def test_explicit_adoption_verifies_binaries_and_preserves_existing_debug_symbols(self):
        sherpa, ort = self.windows_archives()
        output = self.deps / "native/test"
        with patch.dict(runtime.ARTIFACTS, {"windows-x64": (sherpa[1], ort[1])}):
            runtime.prepare("windows-x64", output, True, self.deps)
            (output / runtime.MANIFEST).unlink()
            symbols = output / "lib/onnxruntime.pdb"
            symbols.write_bytes(b"existing-symbols")
            runtime.prepare("windows-x64", output, True, self.deps, adopt_existing=True)
        self.assertTrue((output / runtime.MANIFEST).exists())
        self.assertEqual(symbols.read_bytes(), b"existing-symbols")

    def test_hash_and_size_mismatch_fail_before_extraction(self):
        archive, artifact = self.archive("file.tar.bz2", [("root/lib/test.so", b"library")])
        for altered in [runtime.Artifact(artifact.name, artifact.size + 1, artifact.sha256, artifact.url), runtime.Artifact(artifact.name, artifact.size, "0" * 64, artifact.url)]:
            with self.subTest(artifact=altered), self.assertRaisesRegex(ValueError, "mismatch"):
                runtime.verify_archive(archive, altered)

    def test_download_is_verified_before_atomic_cache_reuse(self):
        data = b"verified-archive"
        artifact = runtime.Artifact("download.tgz", len(data), runtime.hashlib.sha256(data).hexdigest(), "https://example.invalid/download.tgz")
        with patch.object(runtime.urllib.request, "urlopen", return_value=io.BytesIO(data)) as network:
            first = runtime.cached_archive(artifact, False, self.deps)
            second = runtime.cached_archive(artifact, True, self.deps)
        self.assertEqual(first, second)
        self.assertEqual(first.read_bytes(), data)
        self.assertEqual(network.call_count, 1)
        self.assertFalse(list(first.parent.glob("*.part")))
        bad = runtime.Artifact("bad.tgz", len(data), "0" * 64, artifact.url)
        with patch.object(runtime.urllib.request, "urlopen", return_value=io.BytesIO(data)), self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            runtime.cached_archive(bad, False, self.deps)
        self.assertFalse((first.parent / bad.name).exists())
        self.assertFalse(list(first.parent.glob("*.part")))

    def test_archive_paths_cannot_escape_on_either_platform(self):
        for name in ["../outside.dll", "/outside.dll", "C:/outside.dll", "root\\lib\\outside.dll", "root/lib/../outside.dll"]:
            for kind in ["tar", "zip"]:
                with self.subTest(name=name, kind=kind):
                    archive, _ = self.archive("unsafe." + kind, [(name, b"unsafe")], kind)
                    with self.assertRaisesRegex(ValueError, "Unsafe archive path"):
                        runtime.extract_libraries(archive, self.deps / kind, "windows-x64", False)

    def test_linux_relative_soname_links_and_hardlinks_preserve_library_bytes(self):
        archive, _ = self.archive("linux.tar.bz2", [
            ("root/lib/libonnxruntime.so.1.30.0", b"native-library"),
            ("root/lib/libonnxruntime.so.1", (tarfile.SYMTYPE, "libonnxruntime.so.1.30.0")),
            ("root/lib/libonnxruntime.so", (tarfile.SYMTYPE, "libonnxruntime.so.1")),
            ("root/lib/libextra.so", (tarfile.LNKTYPE, "root/lib/libonnxruntime.so.1.30.0")),
        ])
        libraries = self.deps / "lib"
        runtime.extract_libraries(archive, libraries, "linux-x64", False)
        for name in ["libonnxruntime.so", "libonnxruntime.so.1", "libextra.so"]:
            self.assertEqual((libraries / name).read_bytes(), b"native-library")
        if runtime.os.name != "nt":
            self.assertEqual(runtime.os.readlink(libraries / "libonnxruntime.so"), "libonnxruntime.so.1")

    def test_escaping_missing_and_cyclic_native_links_are_rejected(self):
        cases = [
            [("root/lib/libbad.so", (tarfile.SYMTYPE, "../../../outside.so"))],
            [("root/lib/libbad.so", (tarfile.SYMTYPE, "missing.so"))],
            [("root/lib/libbad.so", (tarfile.SYMTYPE, "libbad.so"))],
        ]
        for index, entries in enumerate(cases):
            with self.subTest(index=index):
                archive, _ = self.archive(f"bad-links-{index}.tar.bz2", entries)
                with self.assertRaises(ValueError):
                    runtime.extract_libraries(archive, self.deps / f"links-{index}", "linux-x64", False)

    def test_output_paths_must_stay_in_the_deps_directory(self):
        for output in [self.deps.parent / "outside-runtime", self.deps]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                runtime.prepare("windows-x64", output, True, self.deps)


if __name__ == "__main__":
    unittest.main()
