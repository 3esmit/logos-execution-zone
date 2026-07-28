# Changelog

All notable changes to this fork are recorded here.

## Unreleased

## 0.4.0-alpha.3 - 2026-07-28

### Added

- `wallet auth-transfer send --submit-only` submits public-to-public transfers
  and returns the transaction hash without waiting for finalization. Private
  and mixed-privacy transfers remain explicitly unsupported in this mode.

## 0.4.0-alpha.2 - 2026-07-27

### Fixed

- Native wallet and wallet FFI releases now use the immutable Testnet v0.2
  public-transfer program-ID profile and report incompatible program catalogs
  as errors instead of panicking.
- Wallet cold starts use a bounded calibration default when historical
  statistics are not yet available.

## 0.4.0-alpha.1 - 2026-07-24

### Added

- Source-owned Linux AMD64 and Apple silicon macOS operator release bundles.
- Native bundle manifests, checksums, deterministic packaging, and native smoke
  tests.
- Fork-owned GHCR publication for sequencer, indexer, and explorer images.

### Fixed

- Existing single-sequencer wallet configurations now load through the
  multi-sequencer wallet.
- Public transactions no longer require the optional bulk proof RPC.
