# Testnet v0.2 program profile

The `testnet_initial_state/testnet-v0.2` profile preserves the program
artifacts and ProgramIds deployed by the public Testnet v0.2 network. The
profile is selected explicitly by the `testnet-v0-2` feature used by native
wallet builds and by the Testnet initial-state loader; development artifacts
remain separate.

`manifest.json` records the deployment source revision, each artifact digest,
and each deployed image ID. `lez/programs/src/testnet.rs` binds those IDs to
the checked-in bytecode. The profile gate checks all three values together:

```sh
python3 scripts/check_testnet_program_profile.py
python3 scripts/test_testnet_program_profile.py
```

The gate rejects a changed binary, changed ProgramId, missing artifact,
duplicate or reordered manifest entry, malformed digest, or missing workflow
invocation. A guest-toolchain update therefore cannot publish a release until
the deployment identity is intentionally re-attested in the profile.

This repository preserves the deployed profile. A future Testnet migration
must add a new versioned profile and deployment evidence rather than replacing
these artifacts in place.
