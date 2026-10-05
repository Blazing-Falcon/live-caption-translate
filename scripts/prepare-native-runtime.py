#!/usr/bin/env python3
"""Prepare pinned sherpa/ONNX Runtime libraries using only Python's stdlib.

Run from any directory: python scripts/prepare-native-runtime.py [--json]
The default stdout is the SHERPA_ONNX_LIB_DIR path. All downloads, temporary
files and extracted libraries stay in the deps directory: LT_DEPS_DIR when set,
otherwise .deps in this repository.
No native library is executed. --offline requires already verified archives.

The pinned sherpa build excludes TTS. Earlier TTS-enabled development artifacts
statically included GPLv3 eSpeak NG; leaving TTS unused does not remove those
redistribution obligations. Existing directories are verified and never upgraded
in place. Prepare into the new no-tts default or an explicit fresh --out, then
coordinate runtime installation after any process using the old DLLs has stopped.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import stat
import sys
import tarfile
import tempfile
from dataclasses import dataclass
from typing import BinaryIO
import urllib.request
import zipfile


WORKSPACE = Path(__file__).resolve().parents[1]
DEPS = Path(os.environ.get("LT_DEPS_DIR") or WORKSPACE / ".deps").resolve()
SHERPA_VERSION = "1.13.8"
SHERPA_BUILD = "no-tts"
ORT_VERSION = "1.30.0"
MANIFEST = ".lt-native-runtime.json"


@dataclass(frozen=True)
class Artifact:
    name: str
    size: int
    sha256: str
    url: str


ARTIFACTS = {
    "windows-x64": (
        Artifact(
            "sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-no-tts-lib.tar.bz2",
            7536080,
            "a1253e665c4f236119c443c8932a8acfca32a546c78c05962d483c9a0eae21b7",
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/"
            "sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-no-tts-lib.tar.bz2",
        ),
        Artifact(
            "onnxruntime-win-x64-1.30.0.zip",
            82645522,
            "c6ba983baf5681af108599675d2a89c2d145512d02de28aed0bff177cd0ba949",
            "https://github.com/microsoft/onnxruntime/releases/download/v1.30.0/"
            "onnxruntime-win-x64-1.30.0.zip",
        ),
    ),
    "linux-x64": (
        Artifact(
            "sherpa-onnx-v1.13.8-linux-x64-shared-no-tts-lib.tar.bz2",
            9249201,
            "bf2d998c8b07012cd5098f3b92673bc1333fd9b927767d7cb664be8190d8bc0b",
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/"
            "sherpa-onnx-v1.13.8-linux-x64-shared-no-tts-lib.tar.bz2",
        ),
        Artifact(
            "onnxruntime-linux-x64-1.30.0.tgz",
            11306877,
            "a5ed5a3cac51fbb2e90da632ae43d19212faaa20e76484e62bcb7c23ddb3b3fd",
            "https://github.com/microsoft/onnxruntime/releases/download/v1.30.0/"
            "onnxruntime-linux-x64-1.30.0.tgz",
        ),
    ),
}


def within_deps(path: Path, deps: Path = DEPS) -> Path:
    path = path.resolve()
    if not path.is_relative_to(deps.resolve()):
        raise ValueError(f"Path is outside the deps directory: {path}")
    return path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_archive(path: Path, artifact: Artifact) -> None:
    if not path.is_file() or path.stat().st_size != artifact.size:
        raise ValueError(f"Archive size mismatch: {path} (expected {artifact.size} bytes)")
    if sha256(path) != artifact.sha256:
        raise ValueError(f"Archive SHA-256 mismatch: {path}")


def cached_archive(artifact: Artifact, offline: bool, deps: Path = DEPS) -> Path:
    cache = within_deps(deps / "downloads", deps)
    candidate = cache / artifact.name
    if candidate.exists():
        verify_archive(candidate, artifact)
        print(f"Verified cached {artifact.name}", file=sys.stderr)
        return candidate
    if offline:
        raise ValueError(f"Verified archive is not cached: {artifact.name}")
    cache.mkdir(parents=True, exist_ok=True)
    print(f"Downloading {artifact.url}", file=sys.stderr)
    request = urllib.request.Request(artifact.url, headers={"User-Agent": "live-translator-runtime-setup"})
    with tempfile.NamedTemporaryFile(dir=cache, prefix=artifact.name + ".", suffix=".part", delete=False) as temporary:
        download = Path(temporary.name)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                size = 0
                while block := response.read(1 << 20):
                    size += len(block)
                    if size > artifact.size:
                        raise ValueError(f"Download exceeds pinned size: {artifact.name}")
                    temporary.write(block)
            temporary.flush()
            os.fsync(temporary.fileno())
        except BaseException:
            temporary.close()
            download.unlink(missing_ok=True)
            raise
    try:
        verify_archive(download, artifact)
        destination = cache / artifact.name
        if destination.exists():
            verify_archive(destination, artifact)
            download.unlink()
        else:
            download.replace(destination)
        return destination
    finally:
        download.unlink(missing_ok=True)


def archive_path(name: str) -> PurePosixPath:
    # Backslashes and drive prefixes can become separators/absolute paths on
    # Windows even though archive filenames conventionally use POSIX paths.
    if not name or "\\" in name or ":" in name or "\0" in name:
        raise ValueError(f"Unsafe archive path: {name!r}")
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"Unsafe archive path: {name!r}")
    return path


def library_path(path: PurePosixPath, target: str) -> PurePosixPath | None:
    if len(path.parts) < 3 or path.parts[1] != "lib":
        return None
    name = path.name.lower()
    if target == "windows-x64":
        selected = name.endswith((".dll", ".lib"))
    else:
        selected = name.endswith(".so") or ".so." in name
    return PurePosixPath(*path.parts[2:]) if selected else None


def link_target(path: PurePosixPath, target: str, hardlink: bool) -> PurePosixPath:
    if not target or "\\" in target or ":" in target or "\0" in target:
        raise ValueError(f"Unsafe archive link: {path} -> {target!r}")
    target_path = PurePosixPath(target)
    if target_path.is_absolute():
        raise ValueError(f"Absolute archive link: {path} -> {target}")
    parts = [] if hardlink else list(path.parent.parts)
    for part in target_path.parts:
        if part == "..":
            if not parts:
                raise ValueError(f"Archive link escapes its root: {path} -> {target}")
            parts.pop()
        elif part != ".":
            parts.append(part)
    return PurePosixPath(*parts)


def copy_member(source: BinaryIO, destination: Path, mode: int) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("xb") as output:
        shutil.copyfileobj(source, output, length=1 << 20)
    if os.name != "nt":
        destination.chmod((mode & 0o777) | stat.S_IRUSR | stat.S_IWUSR)


def extract_libraries(archive: Path, lib_dir: Path, target: str, skip_old_ort: bool) -> None:
    """Extract selected native members manually; never use extractall()."""
    lib_dir.mkdir(parents=True, exist_ok=True)
    links: dict[PurePosixPath, tuple[PurePosixPath, bool]] = {}
    selected: set[PurePosixPath] = set()

    def choose(name: str) -> tuple[PurePosixPath, PurePosixPath] | None:
        original = archive_path(name)
        relative = library_path(original, target)
        if relative is None:
            return None
        # Discard every old runtime/provider library, including versioned .so
        # files and links; Microsoft supplies the complete replacement set.
        if skip_old_ort and relative.name.lower().startswith(("onnxruntime", "libonnxruntime")):
            return None
        if relative in selected:
            raise ValueError(f"Duplicate native archive member: {name}")
        selected.add(relative)
        return original, relative

    def remember_link(original: PurePosixPath, relative: PurePosixPath, name: str, hardlink: bool) -> None:
        destination = library_path(link_target(original, name, hardlink), target)
        if destination is None:
            raise ValueError(f"Native link leaves the library directory: {original} -> {name}")
        links[relative] = destination, hardlink

    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as contents:
            for member in contents.infolist():
                # ZipInfo normalizes backslashes on Windows; validate its raw
                # original name before using the normalized filename.
                member_path = archive_path(member.orig_filename)
                mode = member.external_attr >> 16
                if stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR, stat.S_IFLNK):
                    raise ValueError(f"Unsupported archive member: {member_path}")
                if member.is_dir():
                    continue
                item = choose(member.filename)
                if item is None:
                    continue
                original, relative = item
                if stat.S_ISLNK(mode):
                    remember_link(original, relative, contents.read(member).decode("utf-8"), False)
                else:
                    with contents.open(member) as source:
                        copy_member(source, lib_dir.joinpath(*relative.parts), mode)
    else:
        with tarfile.open(archive, "r:*") as contents:
            for member in contents:
                archive_path(member.name)
                if not (member.isfile() or member.isdir() or member.issym() or member.islnk()):
                    raise ValueError(f"Unsupported archive member: {member.name}")
                if member.isdir():
                    continue
                item = choose(member.name)
                if item is None:
                    continue
                original, relative = item
                if member.issym() or member.islnk():
                    remember_link(original, relative, member.linkname, member.islnk())
                else:
                    source = contents.extractfile(member)
                    if source is None:
                        raise ValueError(f"Cannot read archive member: {member.name}")
                    with source:
                        copy_member(source, lib_dir.joinpath(*relative.parts), member.mode)

    def resolved_file(relative: PurePosixPath, visiting: set[PurePosixPath]) -> PurePosixPath:
        if relative not in selected:
            raise ValueError(f"Native link references an unselected file: {relative}")
        if relative in visiting:
            raise ValueError(f"Native archive contains a link cycle: {relative}")
        if relative in links:
            return resolved_file(links[relative][0], visiting | {relative})
        return relative

    for relative, (destination, hardlink) in links.items():
        final = lib_dir.joinpath(*resolved_file(relative, set()).parts)
        output = lib_dir.joinpath(*relative.parts)
        output.parent.mkdir(parents=True, exist_ok=True)
        if hardlink:
            os.link(final, output)
        elif os.name == "nt":
            # Cross-target preparation on Windows does not require permission
            # to create symlinks. Both SONAME aliases retain the same bytes.
            shutil.copyfile(final, output)
        else:
            output.symlink_to(os.path.relpath(lib_dir.joinpath(*destination.parts), output.parent))


def inventory(lib_dir: Path, target: str) -> list[dict]:
    result = []
    for path in sorted(lib_dir.rglob("*")):
        relative = path.relative_to(lib_dir).as_posix()
        if library_path(PurePosixPath("runtime/lib") / relative, target) is None:
            continue
        if path.is_symlink():
            resolved = path.resolve(strict=True)
            if not resolved.is_relative_to(lib_dir.resolve()):
                raise ValueError(f"Native symlink leaves its library directory: {path}")
            result.append({"path": relative, "link": os.readlink(path)})
        elif path.is_file():
            result.append({"path": relative, "size": path.stat().st_size, "sha256": sha256(path)})
    return result


def require_libraries(lib_dir: Path, target: str) -> None:
    required = (
        ["sherpa-onnx-c-api.dll", "sherpa-onnx-c-api.lib", "onnxruntime.dll", "onnxruntime.lib"]
        if target == "windows-x64"
        else ["libsherpa-onnx-c-api.so", "libonnxruntime.so", "libonnxruntime.so.1"]
    )
    if any(not (lib_dir / name).is_file() for name in required):
        raise ValueError(f"Pinned archives lack required {target} libraries: {required}")


def prepare(target: str, output: Path, offline: bool, deps: Path = DEPS, adopt_existing: bool = False) -> dict:
    output = within_deps(output, deps)
    if output == deps.resolve():
        raise ValueError("The managed output must be a directory inside the deps directory")
    artifacts = ARTIFACTS[target]
    expected = {
        "schema": 2,
        "platform": target,
        "sherpa_version": SHERPA_VERSION,
        "sherpa_build": SHERPA_BUILD,
        "onnxruntime_version": ORT_VERSION,
        "archives": [artifact.__dict__ for artifact in artifacts],
    }
    lib_dir = within_deps(output / "lib", deps)
    manifest_path = output / MANIFEST
    populated = output.exists() and any(output.iterdir())
    if populated and manifest_path.is_file():
        require_libraries(lib_dir, target)
        existing = json.loads(manifest_path.read_text(encoding="utf-8"))
        if any(existing.get(key) != value for key, value in expected.items()) or existing.get("libraries") != inventory(lib_dir, target):
            raise ValueError(f"Managed runtime is modified or incomplete, or uses legacy pinned artifacts; existing files are preserved. Choose a new --out directory for the no-TTS runtime: {output}")
        expected = existing
        print(f"Verified prepared runtime: {lib_dir}", file=sys.stderr)
    else:
        if populated and not adopt_existing:
            raise ValueError(f"Refusing to replace an unmanaged directory: {output}")
        archives = [cached_archive(artifact, offline, deps) for artifact in artifacts]
        temporary_root = within_deps(deps / "tmp", deps)
        temporary_root.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=temporary_root, prefix="native-runtime-") as staging:
            staged = Path(staging) / "runtime"
            staged_lib = staged / "lib"
            extract_libraries(archives[0], staged_lib, target, skip_old_ort=True)
            extract_libraries(archives[1], staged_lib, target, skip_old_ort=False)
            require_libraries(staged_lib, target)
            expected["libraries"] = inventory(staged_lib, target)
            (staged / MANIFEST).write_text(json.dumps(expected, indent=2) + "\n", encoding="utf-8")
            output.parent.mkdir(parents=True, exist_ok=True)
            if populated:
                if inventory(lib_dir, target) != expected["libraries"]:
                    raise ValueError(f"Existing native libraries do not match the pinned archives: {lib_dir}")
                # Adoption writes only metadata. In-use native files and any
                # existing debug symbols are never overwritten or removed.
                shutil.copyfile(staged / MANIFEST, manifest_path)
            else:
                if output.exists():
                    output.rmdir()  # Only an empty directory can be replaced.
                staged.replace(output)
        print(f"Prepared runtime: {lib_dir}", file=sys.stderr)
    runtime = lib_dir / ("onnxruntime.dll" if target == "windows-x64" else "libonnxruntime.so")
    # runtime_path describes the prepared source library. Do not export it as
    # LT_ONNXRUNTIME: dev builds copy DLLs beside executables, and an explicit
    # source path could then create a second runtime instance in that process.
    return {**expected, "lib_dir": str(lib_dir), "SHERPA_ONNX_LIB_DIR": str(lib_dir), "runtime_path": str(runtime)}


def default_output(target: str) -> Path:
    prefix = "" if target == "windows-x64" else target + "-"
    return Path(f"native/{prefix}sherpa-{SHERPA_VERSION}-{SHERPA_BUILD}-ort-{ORT_VERSION}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=ARTIFACTS, help="Defaults to the host Windows/Linux x64 platform")
    parser.add_argument("--out", type=Path, help="Managed output directory inside the deps directory")
    parser.add_argument("--offline", action="store_true", help="Require cached verified archives; never access the network")
    parser.add_argument("--adopt-existing", action="store_true", help="Verify an unmanaged directory against pinned archives and write its manifest without replacing libraries")
    parser.add_argument("--json", action="store_true", help="Emit library paths and exact archive metadata as JSON")
    args = parser.parse_args()
    target = args.platform
    if target is None:
        machine = platform.machine().lower()
        system = platform.system().lower()
        if machine not in ("x86_64", "amd64") or system not in ("windows", "linux"):
            parser.error("Only Windows/Linux x64 are supported; specify --platform to prepare another target")
        target = f"{system}-x64"
    output = args.out or default_output(target)
    if not output.is_absolute():
        output = DEPS / output
    try:
        result = prepare(target, output, args.offline, adopt_existing=args.adopt_existing)
    except (OSError, ValueError, tarfile.TarError, zipfile.BadZipFile) as error:
        parser.exit(1, f"Native runtime preparation failed: {error}\n")
    print(json.dumps(result) if args.json else result["lib_dir"])


if __name__ == "__main__":
    main()
