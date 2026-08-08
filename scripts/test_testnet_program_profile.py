#!/usr/bin/env python3
"""Regression tests for the Testnet profile attestation gate."""

from __future__ import annotations

import importlib.util
import json
import struct
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check_testnet_program_profile.py")
SPEC = importlib.util.spec_from_file_location("check_testnet_program_profile", SCRIPT)
assert SPEC and SPEC.loader
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


def write_fixture(root: Path) -> None:
    profile = root / CHECK.PROFILE_DIR
    profile.mkdir(parents=True)
    source = root / CHECK.SOURCE_PATH
    source.parent.mkdir(parents=True)
    source.write_text(
        "pub const AUTHENTICATED_TRANSFER_ID: [u32; 8] = [1, 2, 3, 4, 5, 6, 7, 8];\n",
        encoding="utf-8",
    )
    binary = b"testnet-profile-fixture"
    (profile / "authenticated_transfer.bin").write_bytes(binary)
    manifest = {
        "network": "testnet",
        "version": "v0.2",
        "source_revision": "0123456789abcdef0123456789abcdef01234567",
        "artifacts": [
            {
                "name": "authenticated_transfer",
                "image_id": struct.pack("<8I", 1, 2, 3, 4, 5, 6, 7, 8).hex(),
                "sha256": CHECK.sha256(binary).hexdigest(),
            }
        ],
    }
    (profile / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")


class TestTestnetProgramProfile(unittest.TestCase):
    def test_accepts_matching_manifest_source_and_binary(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = CHECK.EXPECTED_ARTIFACTS
            CHECK.EXPECTED_ARTIFACTS = {"authenticated_transfer": "AUTHENTICATED_TRANSFER"}
            try:
                write_fixture(root)
                CHECK.validate_profile(root)
            finally:
                CHECK.EXPECTED_ARTIFACTS = original

    def test_rejects_binary_digest_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = CHECK.EXPECTED_ARTIFACTS
            CHECK.EXPECTED_ARTIFACTS = {"authenticated_transfer": "AUTHENTICATED_TRANSFER"}
            try:
                write_fixture(root)
                (root / CHECK.PROFILE_DIR / "authenticated_transfer.bin").write_bytes(b"changed")
                with self.assertRaisesRegex(ValueError, "sha256"):
                    CHECK.validate_profile(root)
            finally:
                CHECK.EXPECTED_ARTIFACTS = original

    def test_rejects_program_id_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = CHECK.EXPECTED_ARTIFACTS
            CHECK.EXPECTED_ARTIFACTS = {"authenticated_transfer": "AUTHENTICATED_TRANSFER"}
            try:
                write_fixture(root)
                (root / CHECK.SOURCE_PATH).write_text(
                    "pub const AUTHENTICATED_TRANSFER_ID: [u32; 8] = [9, 2, 3, 4, 5, 6, 7, 8];\n",
                    encoding="utf-8",
                )
                with self.assertRaisesRegex(ValueError, "ProgramId"):
                    CHECK.validate_profile(root)
            finally:
                CHECK.EXPECTED_ARTIFACTS = original


if __name__ == "__main__":
    unittest.main()
