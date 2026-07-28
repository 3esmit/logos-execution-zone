#!/usr/bin/env python3
"""Static contract checks for source-owned release workflows."""

from __future__ import annotations

import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def require(text: str, needle: str, source: Path) -> None:
    if needle not in text:
        raise ValueError(f"{source}: missing required release contract: {needle}")


def forbid(text: str, needle: str, source: Path) -> None:
    if needle in text:
        raise ValueError(f"{source}: forbidden release contract: {needle}")


def check_images() -> None:
    source = ROOT / ".github/workflows/publish_images.yml"
    text = source.read_text(encoding="utf-8")
    for needle in (
        "packages: write",
        "registry: ghcr.io",
        "username: ${{ github.actor }}",
        "password: ${{ github.token }}",
        "ghcr.io/${{ github.repository }}/risc0_base",
        "ghcr.io/${{ github.repository }}/${{ matrix.name }}",
        'tags:\n      - "v*"',
    ):
        require(text, needle, source)
    for needle in (
        "secrets.DOCKER_REGISTRY",
        "secrets.DOCKER_USERNAME",
        "secrets.DOCKER_PASSWORD",
    ):
        forbid(text, needle, source)


def check_native() -> None:
    source = ROOT / ".github/workflows/publish_native.yml"
    text = source.read_text(encoding="utf-8")
    for needle in (
        'pull_request:\n    paths:',
        '"CHANGELOG.md"',
        '"Cargo.lock"',
        '"Cargo.toml"',
        '"flake.nix"',
        '"lez/explorer_service/Cargo.toml"',
        '"lez/indexer/ffi/Cargo.toml"',
        '"lez/indexer/service/Cargo.toml"',
        '"lez/sequencer/service/Cargo.toml"',
        '"lez/wallet-ffi/Cargo.toml"',
        '"lez/wallet/Cargo.toml"',
        'tags:\n      - "v*"',
        "runs-on: ubuntu-24.04",
        "runs-on: macos-latest",
        "linux-amd64",
        "darwin-arm64",
        "GITHUB_TOKEN: ${{ github.token }}",
        'cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"',
        '"$cargo_bin/r0vm" --version',
        "scripts/build-native-release.sh",
        "scripts/native_release.py validate-source",
        "scripts/native_release.py write-checksums",
        "scripts/native_release.py verify-release",
        "draft: true",
        "id: create_release",
        "steps.create_release.outputs.release_id",
        '"repos/$GITHUB_REPOSITORY/releases/$RELEASE_ID"',
        '"repos/$GITHUB_REPOSITORY/releases/$RELEASE_ID/assets?per_page=100"',
        '"https://api.github.com/repos/$GITHUB_REPOSITORY/releases/assets/$asset_id"',
        "draft: false",
        "Roll back only exact owned draft",
        "release-artifact/*.tar.gz",
        "release-artifact/SHA256SUMS",
        "contents: write",
    ):
        require(text, needle, source)
    for needle in (
        "RISC0_SKIP_BUILD_KERNELS",
        "RISC0_SKIP_BUILD=1",
        "secrets.DOCKER_",
        "softprops/action-gh-release",
        "gh release delete",
        "--cleanup-tag",
    ):
        forbid(text, needle, source)


def check_native_build_profile() -> None:
    build_script = ROOT / "scripts/build-native-release.sh"
    build_text = build_script.read_text(encoding="utf-8")
    for needle in (
        "--package wallet --bin wallet --features testnet-v0-2",
        "--package wallet-ffi --features testnet-v0-2",
        "bundle_python_runtime",
        "verify_extracted_wallet_runtime",
        "link-arg=-Wl,-rpath,$ORIGIN/../lib",
        "link-arg=-Wl,-rpath,$ORIGIN",
        "install_name_tool -change",
        "@loader_path/../lib",
        "@loader_path",
    ):
        require(build_text, needle, build_script)

    flake = ROOT / "flake.nix"
    require(
        flake.read_text(encoding="utf-8"),
        'cargoExtraArgs = "-p wallet-ffi --features testnet-v0-2"',
        flake,
    )


def main() -> int:
    try:
        check_images()
        check_native()
        check_native_build_profile()
    except (OSError, ValueError) as error:
        print(f"release workflow validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
