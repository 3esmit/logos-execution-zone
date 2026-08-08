#!/usr/bin/env python3
"""Validate the checked-in artifacts and IDs for the deployed Testnet profile."""

from __future__ import annotations

import json
import re
import struct
import sys
from hashlib import sha256
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PROFILE_DIR = Path("lez/testnet_initial_state/testnet-v0.2")
SOURCE_PATH = Path("lez/programs/src/testnet.rs")

EXPECTED_ARTIFACTS = {
    "authenticated_transfer": "AUTHENTICATED_TRANSFER",
    "token": "TOKEN",
    "amm": "AMM",
    "clock": "CLOCK",
    "associated_token_account": "ASSOCIATED_TOKEN_ACCOUNT",
    "vault": "VAULT",
    "faucet": "FAUCET",
    "bridge": "BRIDGE",
    "pinata": "PINATA",
    "privacy_preserving_circuit": "PRIVACY_PRESERVING_CIRCUIT",
}

HEX_RE = re.compile(r"^[0-9a-f]{64}$")
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")


def _fail(message: str) -> None:
    raise ValueError(message)


def _parse_id(source: str, constant: str, source_path: Path) -> str:
    match = re.search(
        rf"pub const {re.escape(constant)}_ID: \[u32; 8\] = \[(.*?)\];",
        source,
        re.DOTALL,
    )
    if match is None:
        _fail(f"missing {constant}_ID in {source_path}")
    values = [int(value.replace("_", "")) for value in re.findall(r"\d[\d_]*", match.group(1))]
    if len(values) != 8 or any(value > 0xFFFF_FFFF for value in values):
        _fail(f"{constant}_ID must contain eight u32 values")
    return struct.pack("<8I", *values).hex()


def validate_profile(root: Path = ROOT) -> None:
    manifest_path = root / PROFILE_DIR / "manifest.json"
    source_path = root / SOURCE_PATH
    profile_dir = root / PROFILE_DIR

    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        _fail(f"cannot read {manifest_path}: {error}")

    if manifest.get("network") != "testnet" or manifest.get("version") != "v0.2":
        _fail(f"{manifest_path}: expected deployed testnet v0.2 profile")
    source_revision = manifest.get("source_revision")
    if not isinstance(source_revision, str) or REVISION_RE.fullmatch(source_revision) is None:
        _fail(f"{manifest_path}: source_revision must be a 40-character lowercase commit")

    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, list):
        _fail(f"{manifest_path}: artifacts must be a list")
    names = [entry.get("name") for entry in artifacts if isinstance(entry, dict)]
    if names != list(EXPECTED_ARTIFACTS):
        _fail(f"{manifest_path}: artifact order/set must be {list(EXPECTED_ARTIFACTS)}")

    try:
        source = source_path.read_text(encoding="utf-8")
    except OSError as error:
        _fail(f"cannot read {source_path}: {error}")

    for entry in artifacts:
        if not isinstance(entry, dict):
            _fail(f"{manifest_path}: every artifact entry must be an object")
        name = entry.get("name")
        image_id = entry.get("image_id")
        expected_sha = entry.get("sha256")
        if name not in EXPECTED_ARTIFACTS:
            _fail(f"{manifest_path}: unsupported artifact {name!r}")
        if not isinstance(image_id, str) or HEX_RE.fullmatch(image_id) is None:
            _fail(f"{manifest_path}: {name} image_id must be 64 lowercase hex characters")
        if not isinstance(expected_sha, str) or HEX_RE.fullmatch(expected_sha) is None:
            _fail(f"{manifest_path}: {name} sha256 must be 64 lowercase hex characters")

        binary_path = profile_dir / f"{name}.bin"
        try:
            binary = binary_path.read_bytes()
        except OSError as error:
            _fail(f"cannot read {binary_path}: {error}")
        actual_sha = sha256(binary).hexdigest()
        if actual_sha != expected_sha:
            _fail(f"{binary_path}: sha256 {actual_sha} does not match manifest {expected_sha}")

        actual_id = _parse_id(source, EXPECTED_ARTIFACTS[name], source_path)
        if actual_id != image_id:
            _fail(f"{source_path}: {name} ProgramId {actual_id} does not match manifest {image_id}")


def main() -> int:
    try:
        validate_profile()
    except (OSError, ValueError) as error:
        print(f"Testnet program profile validation failed: {error}", file=sys.stderr)
        return 1
    print("Testnet program profile is internally attested")
    return 0


if __name__ == "__main__":
    sys.exit(main())
