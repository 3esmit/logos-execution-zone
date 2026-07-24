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
        "gh release download",
        "draft=false",
        "release-artifact/*.tar.gz",
        "release-artifact/SHA256SUMS",
        "contents: write",
    ):
        require(text, needle, source)
    for needle in (
        "RISC0_SKIP_BUILD_KERNELS",
        "RISC0_SKIP_BUILD=1",
        "secrets.DOCKER_",
    ):
        forbid(text, needle, source)


def main() -> int:
    try:
        check_images()
        check_native()
    except (OSError, ValueError) as error:
        print(f"release workflow validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
