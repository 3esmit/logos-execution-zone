#!/usr/bin/env python3
"""Regression tests for native release archive contracts."""

from __future__ import annotations

import io
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import native_release


VERSION = "0.4.0-alpha.1"
COMMIT = "0123456789abcdef0123456789abcdef01234567"


class NativeReleaseTest(unittest.TestCase):
    def create_stage(self, root: Path, target: str) -> Path:
        stage = root / f"stage-{target}"
        for relative in native_release.required_files(target):
            path = stage / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(f"fixture:{target}:{relative}\n".encode())
        site = stage / "share/explorer/site/index.html"
        site.parent.mkdir(parents=True, exist_ok=True)
        site.write_text("<!doctype html>\n", encoding="utf-8")
        for relative in native_release.REQUIRED_EXECUTABLES:
            (stage / relative).chmod(0o755)
        return stage

    def build_release(self, root: Path) -> Path:
        output = root / "release"
        for target in sorted(native_release.TARGETS):
            stage = self.create_stage(root, target)
            native_release.pack(stage, output, VERSION, target, COMMIT)
        native_release.write_checksums(output, VERSION, COMMIT)
        return output

    def create_source_versions(self, root: Path, version: str) -> None:
        for relative in native_release.SOURCE_VERSION_MANIFESTS:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(
                f'[package]\nname = "fixture"\nversion = "{version}"\n',
                encoding="utf-8",
            )

    def test_complete_release_roundtrip(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            release = self.build_release(Path(temporary))
            native_release.verify_release(release, VERSION, COMMIT)

    def test_archive_creation_is_deterministic(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first_stage = self.create_stage(root / "first", "linux-amd64")
            second_stage = self.create_stage(root / "second", "linux-amd64")
            first = native_release.pack(
                first_stage,
                root / "first-output",
                VERSION,
                "linux-amd64",
                COMMIT,
            )
            second = native_release.pack(
                second_stage,
                root / "second-output",
                VERSION,
                "linux-amd64",
                COMMIT,
            )
            self.assertEqual(
                native_release.sha256_file(first),
                native_release.sha256_file(second),
            )

    def test_missing_required_program_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = self.create_stage(root, "linux-amd64")
            (stage / "bin/wallet").unlink()
            with self.assertRaisesRegex(native_release.ReleaseError, "bin/wallet"):
                native_release.pack(
                    stage,
                    root / "output",
                    VERSION,
                    "linux-amd64",
                    COMMIT,
                )

    def test_tampered_published_archive_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            release = self.build_release(Path(temporary))
            archive = release / native_release.archive_name(VERSION, "linux-amd64")
            with archive.open("ab") as handle:
                handle.write(b"tamper")
            with self.assertRaisesRegex(native_release.ReleaseError, "checksum mismatch"):
                native_release.verify_release(release, VERSION, COMMIT)

    def test_path_traversal_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / native_release.archive_name(VERSION, "linux-amd64")
            bundle_root = native_release.root_name(VERSION, "linux-amd64")
            with tarfile.open(archive, "w:gz") as handle:
                info = tarfile.TarInfo(f"{bundle_root}/../escape")
                payload = b"escape"
                info.size = len(payload)
                handle.addfile(info, io.BytesIO(payload))
            with self.assertRaisesRegex(native_release.ReleaseError, "unsafe archive"):
                native_release.verify_archive(
                    archive,
                    expected_version=VERSION,
                    expected_target="linux-amd64",
                    expected_commit=COMMIT,
                )

    def test_invalid_version_is_rejected(self) -> None:
        with self.assertRaisesRegex(native_release.ReleaseError, "semantic version"):
            native_release.validate_version("release-latest")

    def test_native_package_versions_must_match_release(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.create_source_versions(root, VERSION)
            native_release.validate_source_versions(root, VERSION)
            mismatched = root / native_release.SOURCE_VERSION_MANIFESTS[0]
            mismatched.write_text(
                '[package]\nname = "fixture"\nversion = "0.1.0"\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(native_release.ReleaseError, "does not match"):
                native_release.validate_source_versions(root, VERSION)

    def test_unsafe_manifest_paths_are_rejected(self) -> None:
        for path in ("/tmp/escape", "../escape", "share/../../escape"):
            with self.subTest(path=path):
                with self.assertRaisesRegex(native_release.ReleaseError, "unsafe manifest"):
                    native_release.safe_relative_path(path)


if __name__ == "__main__":
    unittest.main()
