# Programs

This crate serves two purposes at once:

1. Provide one entrypoint for `cargo risczero build` to build guest binaries used in LEZ in one shot.
2. Provide access to the built binaries wrapped with `Program` type.

## Binaries

This crate contains binaries taken from sub-directories: one per each program. This binaries are meant to be compiled with `cargo risczero build`. No other use is intended for them.

## Library

You may import this crate as a library but it will only make sense if you enable `artifacts` feature flag.
Enabling this flag will make crate expect that [`binaries`](#binaries) where already built and put in the right place (use `just build-artifacts` for that).

## Why not just `risc0_build::embed_methods()` ?

Because this will either provide non-deterministic guest build or requires Docker.
And forcing to use Docker to build the project is not an option for us especially because we also build Docker images for our services, which would mean we would have to call docker from docker (and this is not really feasible).

`risc0_build::embed_methods()` works well when you don't need deterministic build or Docker is not a problem. This is the case for our tests and we use it there.
