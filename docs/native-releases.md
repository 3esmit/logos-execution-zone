# Native and container releases

Version tags publish operator artifacts from this repository. The release is
currently alpha software: configuration and data formats can change between
tags.

## Native bundles

Each GitHub release contains:

- `logos-execution-zone-<version>-linux-amd64.tar.gz`
- `logos-execution-zone-<version>-darwin-arm64.tar.gz`
- `SHA256SUMS`

Both archives contain:

- `wallet`
- `sequencer_service`
- `sequencer_service-standalone`
- `indexer_service`
- `explorer_service` and its web assets
- the matching `r0vm`
- wallet and indexer FFI shared libraries and C headers
- the Python runtime required by the wallet and wallet FFI, plus its license
- wallet shell completions
- example wallet, sequencer, and indexer configuration
- `release-manifest.json`, with the source commit and per-file checksums
- `share/runtime-dependencies.txt`, recorded on the native build runner

The Linux archive targets x86_64 GNU/Linux. The macOS archive targets unsigned
Apple silicon binaries. Each executable and FFI shared library is loaded or
invoked on its native runner before publication.

Verify the download before extraction:

```bash
sha256sum --check SHA256SUMS
tar -xzf logos-execution-zone-<version>-linux-amd64.tar.gz
```

On macOS, use `shasum -a 256` to compare the value in `SHA256SUMS`.

Add the extracted `bin` directory to `PATH`. This also makes the bundled
`r0vm` available to proof-producing processes:

```bash
export PATH="$PWD/logos-execution-zone-<version>-linux-amd64/bin:$PATH"
wallet --version
sequencer_service --help
indexer_service --help
explorer_service --help
```

The explorer launcher finds the bundled web assets automatically. Override
`INDEXER_RPC_URL`, `LEPTOS_SITE_ADDR`, or `LEPTOS_SITE_ROOT` when required.

The example configuration is for development and Testnet. It includes a public
test signing key. Never use that key for a production sequencer. Copy the
examples to an operator-owned directory, replace node endpoints, channel ID,
signing key, and data paths, then restrict file permissions before startup.

The released wallet CLI and wallet FFI use the immutable `testnet-v0-2`
profile for public account initialization and native-token transfers. Configure
them with a Testnet sequencer endpoint. This profile does not make the bundled
local sequencer or indexer compatible with historical Testnet private state;
run those services only with their separately supported network configuration.

The wallet and wallet FFI load the bundled Python runtime rather than a system
`libpython`. Other Linux system libraries remain listed in
`share/runtime-dependencies.txt`; install any of those that are missing from the
host. The bundled runtime does not include Keycard's virtual environment or
Python packages, so Keycard commands still require the documented Keycard setup.
The macOS archive is unsigned; verify the checksum and source tag before
approving it through macOS security controls.

## Container images

The same `v*` tag publishes the service images in this repository's GHCR
namespace:

```text
ghcr.io/3esmit/logos-execution-zone/sequencer_service:<tag>
ghcr.io/3esmit/logos-execution-zone/sequencer_service-standalone:<tag>
ghcr.io/3esmit/logos-execution-zone/indexer_service:<tag>
ghcr.io/3esmit/logos-execution-zone/explorer_service:<tag>
```

Docker Compose source builds remain available through `docker compose up`.
Container images and native bundles come from the same source tag.

## Maintainer release procedure

1. Update `CHANGELOG.md` and set the wallet, sequencer service, indexer
   service, explorer service, wallet FFI, and indexer FFI Cargo package
   versions to the release version. The workflow rejects a mismatch.
2. Create a signed, annotated semantic-version tag such as
   `v0.4.0-alpha.1`.
3. Push the tag. `Publish Docker Images` and `Publish Native Release` run from
   that exact commit.
4. Native jobs build and smoke-test both supported targets.
5. Publication creates a draft release, downloads all uploaded files, verifies
   archive manifests and `SHA256SUMS`, then makes the release visible.
6. Confirm all GHCR image tags and GitHub assets before announcing the release.

Release workflows refuse to overwrite an existing GitHub release. A failed run
removes only a draft it created; the source tag remains immutable.
