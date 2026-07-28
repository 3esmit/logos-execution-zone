#!/usr/bin/env python3
"""Build and verify immutable native Logos Execution Zone release archives."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import shutil
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path, PurePosixPath
from typing import BinaryIO


SCHEMA_VERSION = 1
TARGETS = {
    "linux-amd64": {
        "libraries": ("lib/libwallet_ffi.so", "lib/libindexer_ffi.so"),
    },
    "darwin-arm64": {
        "libraries": ("lib/libwallet_ffi.dylib", "lib/libindexer_ffi.dylib"),
    },
}
PYTHON_RUNTIME_PATTERNS = {
    "linux-amd64": re.compile(r"^libpython[0-9]+(?:\.[0-9]+)*\.so(?:\.[0-9]+)*$"),
    "darwin-arm64": re.compile(r"^Python$"),
}
COMMON_REQUIRED_FILES = (
    "bin/wallet",
    "bin/sequencer_service",
    "bin/sequencer_service-standalone",
    "bin/indexer_service",
    "bin/explorer_service",
    "bin/r0vm",
    "libexec/explorer_service",
    "include/wallet_ffi.h",
    "include/indexer_ffi.h",
    "share/explorer/Cargo.toml",
    "share/config-examples/wallet_config.json",
    "share/config-examples/sequencer_config.json",
    "share/config-examples/indexer_config.json",
    "share/completions/bash/wallet",
    "share/completions/zsh/_wallet",
    "share/licenses/Python-PSF-2.0.txt",
    "README.md",
    "LICENSE",
)
REQUIRED_EXECUTABLES = (
    "bin/wallet",
    "bin/sequencer_service",
    "bin/sequencer_service-standalone",
    "bin/indexer_service",
    "bin/explorer_service",
    "bin/r0vm",
    "libexec/explorer_service",
)
VERSION_PATTERN = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
COMMIT_PATTERN = re.compile(r"^[0-9a-f]{40}$")
SOURCE_VERSION_MANIFESTS = (
    "lez/wallet/Cargo.toml",
    "lez/sequencer/service/Cargo.toml",
    "lez/indexer/service/Cargo.toml",
    "lez/explorer_service/Cargo.toml",
    "lez/wallet-ffi/Cargo.toml",
    "lez/indexer/ffi/Cargo.toml",
)


class ReleaseError(ValueError):
    """Release contract violation."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_version(version: str) -> None:
    if VERSION_PATTERN.fullmatch(version) is None:
        raise ReleaseError(f"invalid semantic version: {version}")


def validate_commit(commit: str) -> None:
    if COMMIT_PATTERN.fullmatch(commit) is None:
        raise ReleaseError(f"invalid source commit: {commit}")


def validate_target(target: str) -> None:
    if target not in TARGETS:
        expected = ", ".join(sorted(TARGETS))
        raise ReleaseError(f"unsupported target {target}; expected one of: {expected}")


def validate_source_versions(root: Path, version: str) -> None:
    validate_version(version)
    mismatches: list[str] = []
    for relative in SOURCE_VERSION_MANIFESTS:
        manifest_path = root / relative
        try:
            manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as error:
            raise ReleaseError(f"cannot read package version from {relative}: {error}") from error
        package = manifest.get("package")
        actual = package.get("version") if isinstance(package, dict) else None
        if actual != version:
            mismatches.append(f"{relative}={actual!r}")
    if mismatches:
        raise ReleaseError(
            f"release version {version} does not match native packages: "
            + ", ".join(mismatches)
        )


def archive_name(version: str, target: str) -> str:
    validate_version(version)
    validate_target(target)
    return f"logos-execution-zone-{version}-{target}.tar.gz"


def root_name(version: str, target: str) -> str:
    return archive_name(version, target).removesuffix(".tar.gz")


def required_files(target: str) -> tuple[str, ...]:
    validate_target(target)
    libraries = TARGETS[target]["libraries"]
    return tuple(sorted((*COMMON_REQUIRED_FILES, *libraries)))


def bundled_python_runtimes(stage: Path, target: str) -> list[Path]:
    validate_target(target)
    pattern = PYTHON_RUNTIME_PATTERNS[target]
    runtime_dir = stage / "lib"
    if not runtime_dir.is_dir():
        return []
    return [path for path in runtime_dir.iterdir() if path.is_file() and pattern.fullmatch(path.name)]


def regular_files(root: Path, *, include_manifest: bool = False) -> list[Path]:
    result: list[Path] = []
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ReleaseError(f"symlinks are not allowed in native bundles: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ReleaseError(f"special files are not allowed in native bundles: {path}")
        if not include_manifest and path.relative_to(root).as_posix() == "release-manifest.json":
            continue
        result.append(path)
    return result


def validate_stage(stage: Path, target: str) -> None:
    validate_target(target)
    if not stage.is_dir():
        raise ReleaseError(f"stage directory does not exist: {stage}")

    present = {
        path.relative_to(stage).as_posix()
        for path in regular_files(stage, include_manifest=True)
    }
    missing = sorted(set(required_files(target)) - present)
    if missing:
        raise ReleaseError(f"bundle is missing required files: {', '.join(missing)}")

    python_runtimes = bundled_python_runtimes(stage, target)
    if len(python_runtimes) != 1:
        expected = PYTHON_RUNTIME_PATTERNS[target].pattern
        actual = ", ".join(path.name for path in python_runtimes) or "none"
        raise ReleaseError(
            "bundle must contain exactly one Python runtime "
            f"matching {expected}: {actual}"
        )

    site_files = [path for path in present if path.startswith("share/explorer/site/")]
    if not site_files:
        raise ReleaseError("bundle has no explorer site assets")

    for relative in REQUIRED_EXECUTABLES:
        path = stage / relative
        if path.stat().st_mode & 0o111 == 0:
            raise ReleaseError(f"required program is not executable: {relative}")


def build_manifest(stage: Path, version: str, target: str, commit: str) -> dict[str, object]:
    validate_version(version)
    validate_commit(commit)
    validate_stage(stage, target)

    entries: list[dict[str, object]] = []
    for path in regular_files(stage):
        relative = path.relative_to(stage).as_posix()
        entries.append(
            {
                "path": relative,
                "sha256": sha256_file(path),
                "size": path.stat().st_size,
                "executable": bool(path.stat().st_mode & 0o111),
            }
        )

    return {
        "schemaVersion": SCHEMA_VERSION,
        "name": "logos-execution-zone",
        "version": version,
        "tag": f"v{version}",
        "commit": commit,
        "target": target,
        "files": entries,
    }


def normalized_tar_info(path: Path, arcname: str) -> tarfile.TarInfo:
    info = tarfile.TarInfo(arcname)
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    info.mtime = 0
    if path.is_dir():
        info.type = tarfile.DIRTYPE
        info.mode = 0o755
        info.size = 0
    else:
        info.type = tarfile.REGTYPE
        info.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
        info.size = path.stat().st_size
    return info


def add_to_tar(
    archive: tarfile.TarFile,
    path: Path,
    arcname: str,
    file_handle: BinaryIO | None = None,
) -> None:
    info = normalized_tar_info(path, arcname)
    archive.addfile(info, file_handle)


def pack(stage: Path, output_dir: Path, version: str, target: str, commit: str) -> Path:
    manifest = build_manifest(stage, version, target, commit)
    manifest_path = stage / "release-manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir / archive_name(version, target)
    bundle_root = root_name(version, target)

    with output.open("wb") as raw_output:
        with gzip.GzipFile(
            filename="",
            mode="wb",
            compresslevel=9,
            fileobj=raw_output,
            mtime=0,
        ) as compressed:
            with tarfile.open(
                fileobj=compressed,
                mode="w",
                format=tarfile.PAX_FORMAT,
            ) as archive:
                add_to_tar(archive, stage, bundle_root)
                directories = sorted(
                    (path for path in stage.rglob("*") if path.is_dir()),
                    key=lambda path: path.relative_to(stage).as_posix(),
                )
                for directory in directories:
                    relative = directory.relative_to(stage).as_posix()
                    add_to_tar(archive, directory, f"{bundle_root}/{relative}")
                for path in regular_files(stage, include_manifest=True):
                    relative = path.relative_to(stage).as_posix()
                    with path.open("rb") as source:
                        add_to_tar(
                            archive,
                            path,
                            f"{bundle_root}/{relative}",
                            source,
                        )

    verify_archive(output, expected_version=version, expected_target=target, expected_commit=commit)
    return output


def safe_member_path(member_name: str, expected_root: str) -> PurePosixPath:
    path = PurePosixPath(member_name)
    if path.is_absolute() or ".." in path.parts:
        raise ReleaseError(f"unsafe archive member path: {member_name}")
    if not path.parts or path.parts[0] != expected_root:
        raise ReleaseError(f"archive member is outside expected root: {member_name}")
    return path


def safe_relative_path(relative: str) -> PurePosixPath:
    path = PurePosixPath(relative)
    if path.is_absolute() or not path.parts or ".." in path.parts:
        raise ReleaseError(f"unsafe manifest file path: {relative}")
    return path


def extract_verified_members(
    archive: tarfile.TarFile,
    destination: Path,
    expected_root: str,
) -> None:
    seen: set[str] = set()
    for member in archive.getmembers():
        path = safe_member_path(member.name, expected_root)
        relative = Path(*path.parts)
        if member.name in seen:
            raise ReleaseError(f"duplicate archive member: {member.name}")
        seen.add(member.name)
        output = destination / relative
        if member.isdir():
            output.mkdir(parents=True, exist_ok=True)
            output.chmod(0o755)
            continue
        if not member.isfile():
            raise ReleaseError(f"unsupported archive member type: {member.name}")
        output.parent.mkdir(parents=True, exist_ok=True)
        source = archive.extractfile(member)
        if source is None:
            raise ReleaseError(f"cannot read archive member: {member.name}")
        with output.open("wb") as handle:
            shutil.copyfileobj(source, handle)
        output.chmod(member.mode & 0o777)


def load_manifest(root: Path) -> dict[str, object]:
    path = root / "release-manifest.json"
    if not path.is_file():
        raise ReleaseError("bundle has no release-manifest.json")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise ReleaseError(f"invalid release manifest: {error}") from error
    if not isinstance(value, dict):
        raise ReleaseError("release manifest must be a JSON object")
    return value


def verify_manifest_files(root: Path, manifest: dict[str, object], target: str) -> None:
    entries = manifest.get("files")
    if not isinstance(entries, list):
        raise ReleaseError("release manifest files must be an array")

    expected_by_path: dict[str, dict[str, object]] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            raise ReleaseError("release manifest file entries must be objects")
        path = entry.get("path")
        if not isinstance(path, str) or not path:
            raise ReleaseError("release manifest file path must be a non-empty string")
        safe_relative_path(path)
        if path in expected_by_path:
            raise ReleaseError(f"duplicate manifest file path: {path}")
        expected_by_path[path] = entry

    actual_paths = {
        path.relative_to(root).as_posix()
        for path in regular_files(root)
    }
    if set(expected_by_path) != actual_paths:
        missing = sorted(actual_paths - set(expected_by_path))
        unexpected = sorted(set(expected_by_path) - actual_paths)
        raise ReleaseError(
            "release manifest file set mismatch; "
            f"missing entries={missing}, unexpected entries={unexpected}"
        )

    for relative, entry in expected_by_path.items():
        path = root / relative
        expected_hash = entry.get("sha256")
        expected_size = entry.get("size")
        expected_executable = entry.get("executable")
        if expected_hash != sha256_file(path):
            raise ReleaseError(f"checksum mismatch inside bundle: {relative}")
        if expected_size != path.stat().st_size:
            raise ReleaseError(f"size mismatch inside bundle: {relative}")
        actual_executable = bool(path.stat().st_mode & 0o111)
        if expected_executable is not actual_executable:
            raise ReleaseError(f"executable-mode mismatch inside bundle: {relative}")

    validate_stage(root, target)


def verify_archive(
    archive_path: Path,
    *,
    expected_version: str,
    expected_target: str,
    expected_commit: str,
) -> dict[str, object]:
    validate_version(expected_version)
    validate_target(expected_target)
    validate_commit(expected_commit)
    expected_name = archive_name(expected_version, expected_target)
    if archive_path.name != expected_name:
        raise ReleaseError(
            f"unexpected archive name {archive_path.name}; expected {expected_name}"
        )

    expected_root = root_name(expected_version, expected_target)
    with tempfile.TemporaryDirectory(prefix="lez-release-verify-") as temporary:
        destination = Path(temporary)
        try:
            with tarfile.open(archive_path, mode="r:gz") as archive:
                extract_verified_members(archive, destination, expected_root)
        except (tarfile.TarError, OSError) as error:
            raise ReleaseError(f"invalid release archive: {error}") from error

        root = destination / expected_root
        manifest = load_manifest(root)
        expected_fields = {
            "schemaVersion": SCHEMA_VERSION,
            "name": "logos-execution-zone",
            "version": expected_version,
            "tag": f"v{expected_version}",
            "commit": expected_commit,
            "target": expected_target,
        }
        for field, expected in expected_fields.items():
            if manifest.get(field) != expected:
                raise ReleaseError(
                    f"manifest {field} mismatch: {manifest.get(field)!r} != {expected!r}"
                )
        verify_manifest_files(root, manifest, expected_target)
        return manifest


def expected_archives(version: str) -> tuple[str, ...]:
    return tuple(archive_name(version, target) for target in sorted(TARGETS))


def write_checksums(directory: Path, version: str, commit: str) -> Path:
    validate_commit(commit)
    archives = expected_archives(version)
    present = sorted(path.name for path in directory.glob("*.tar.gz"))
    if present != sorted(archives):
        raise ReleaseError(
            f"release archive set mismatch: found {present}, expected {sorted(archives)}"
        )
    lines: list[str] = []
    for target in sorted(TARGETS):
        name = archive_name(version, target)
        path = directory / name
        verify_archive(
            path,
            expected_version=version,
            expected_target=target,
            expected_commit=commit,
        )
        lines.append(f"{sha256_file(path)}  {name}")
    output = directory / "SHA256SUMS"
    output.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return output


def verify_release(directory: Path, version: str, commit: str) -> None:
    validate_commit(commit)
    checksum_path = directory / "SHA256SUMS"
    if not checksum_path.is_file():
        raise ReleaseError("release has no SHA256SUMS")

    expected = set(expected_archives(version))
    parsed: dict[str, str] = {}
    for line in checksum_path.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^\s/]+)", line)
        if match is None:
            raise ReleaseError(f"invalid SHA256SUMS line: {line!r}")
        digest, name = match.groups()
        if name in parsed:
            raise ReleaseError(f"duplicate SHA256SUMS entry: {name}")
        parsed[name] = digest
    if set(parsed) != expected:
        raise ReleaseError(
            f"SHA256SUMS file set mismatch: found {sorted(parsed)}, expected {sorted(expected)}"
        )

    present_files = {
        path.name
        for path in directory.iterdir()
        if path.is_file()
    }
    allowed_files = expected | {"SHA256SUMS"}
    if present_files != allowed_files:
        raise ReleaseError(
            f"release file set mismatch: found {sorted(present_files)}, "
            f"expected {sorted(allowed_files)}"
        )

    for target in sorted(TARGETS):
        name = archive_name(version, target)
        path = directory / name
        if sha256_file(path) != parsed[name]:
            raise ReleaseError(f"published checksum mismatch: {name}")
        verify_archive(
            path,
            expected_version=version,
            expected_target=target,
            expected_commit=commit,
        )


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser()
    commands = root.add_subparsers(dest="command", required=True)

    validate = commands.add_parser("validate")
    validate.add_argument("--version", required=True)
    validate.add_argument("--commit", required=True)
    validate.add_argument("--target", choices=sorted(TARGETS))

    validate_source = commands.add_parser("validate-source")
    validate_source.add_argument("--root", type=Path, required=True)
    validate_source.add_argument("--version", required=True)

    pack_command = commands.add_parser("pack")
    pack_command.add_argument("--stage", type=Path, required=True)
    pack_command.add_argument("--output-dir", type=Path, required=True)
    pack_command.add_argument("--version", required=True)
    pack_command.add_argument("--target", choices=sorted(TARGETS), required=True)
    pack_command.add_argument("--commit", required=True)

    checksums = commands.add_parser("write-checksums")
    checksums.add_argument("--input-dir", type=Path, required=True)
    checksums.add_argument("--version", required=True)
    checksums.add_argument("--commit", required=True)

    verify = commands.add_parser("verify-release")
    verify.add_argument("--input-dir", type=Path, required=True)
    verify.add_argument("--version", required=True)
    verify.add_argument("--commit", required=True)

    verify_one = commands.add_parser("verify-archive")
    verify_one.add_argument("--archive", type=Path, required=True)
    verify_one.add_argument("--version", required=True)
    verify_one.add_argument("--target", choices=sorted(TARGETS), required=True)
    verify_one.add_argument("--commit", required=True)
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "validate":
            validate_version(args.version)
            validate_commit(args.commit)
            if args.target is not None:
                validate_target(args.target)
        elif args.command == "validate-source":
            validate_source_versions(args.root, args.version)
        elif args.command == "pack":
            output = pack(
                args.stage,
                args.output_dir,
                args.version,
                args.target,
                args.commit,
            )
            print(output)
        elif args.command == "write-checksums":
            output = write_checksums(args.input_dir, args.version, args.commit)
            print(output)
        elif args.command == "verify-release":
            verify_release(args.input_dir, args.version, args.commit)
        elif args.command == "verify-archive":
            verify_archive(
                args.archive,
                expected_version=args.version,
                expected_target=args.target,
                expected_commit=args.commit,
            )
        else:
            raise ReleaseError(f"unsupported command: {args.command}")
    except (ReleaseError, OSError) as error:
        print(f"native release validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
