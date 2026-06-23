set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

# ---- Configuration ----
METHODS_PATH := "program_methods"
TEST_METHODS_PATH := "test_program_methods"
ARTIFACTS := "artifacts"

# Build risc0 program artifacts.
build-artifacts:
    @echo "🔨 Building artifacts"
    @for methods_path in {{METHODS_PATH}} {{TEST_METHODS_PATH}}; do \
        echo "Building artifacts for $methods_path"; \
        CARGO_TARGET_DIR=target/$methods_path cargo risczero build --manifest-path $methods_path/guest/Cargo.toml; \
        mkdir -p {{ARTIFACTS}}/$methods_path; \
        cp target/$methods_path/riscv32im-risc0-zkvm-elf/docker/*.bin {{ARTIFACTS}}/$methods_path; \
    done

# Format codebase.
fmt:
    @echo "🎨 Formatting codebase"
    cargo +nightly fmt
    taplo fmt

# Run tests.
test:
    @echo "🧪 Running tests"
    RISC0_DEV_MODE=1 cargo nextest run --no-fail-fast

# Run criterion benches: fast crypto primitives, then the slow PPE verify (real proving setup).
bench:
    @echo "📊 Running criterion benches"
    cargo bench -p crypto_primitives_bench --bench primitives
    cargo bench -p cycle_bench --features ppe --bench verify

# Run Bedrock node in docker.
[working-directory: 'bedrock']
run-bedrock:
    @echo "⛓️ Running bedrock"
    docker compose up

# Run Sequencer. Run with RISC0_DEV_MODE=1 to disable proof verification for faster iteration.
[working-directory: 'lez/sequencer/service']
run-sequencer standalone="":
    @echo "🧠 Running sequencer"
    @if [ "{{standalone}}" = "standalone" ]; then \
        echo "🧪 Running in standalone mode"; \
        RUST_LOG=info cargo run --features standalone --release -p sequencer_service configs/debug/sequencer_config.json; \
    else \
        echo "🚀 Running in normal mode"; \
        RUST_LOG=info cargo run --release -p sequencer_service configs/debug/sequencer_config.json; \
    fi

# Run Indexer. Run with RISC0_DEV_MODE=1 to disable proof verification for faster iteration.
[working-directory: 'lez/indexer/service']
run-indexer mock="":
    @echo "🔍 Running indexer"
    @if [ "{{mock}}" = "mock" ]; then \
        echo "🧪 Using mock data"; \
        RUST_LOG=info cargo run --release --features mock-responses -p indexer_service configs/debug/indexer_config.json; \
    else \
        echo "🚀 Using real data"; \
        RUST_LOG=info cargo run --release -p indexer_service configs/debug/indexer_config.json; \
    fi

# Run Explorer.
[working-directory: 'lez/explorer_service']
run-explorer:
    @echo "🌐 Running explorer"
    RUST_LOG=info cargo leptos serve

# Run Wallet.
[working-directory: 'lez/wallet']
run-wallet +args:
    @echo "🔑 Running wallet"
    LEE_WALLET_HOME_DIR=$(pwd)/configs/debug cargo run --release -p wallet -- {{args}}

# Import test accounts supplied in sequencer configuration.
wallet-import-test-accounts:
    @echo "⚙️ Initializing accounts"
    just run-wallet account import public --private-key 7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb
    just run-wallet vault claim --account-id Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r --amount 10000

    just run-wallet account import public --private-key f434f8741720014586ae43356d2aec6257da086222f604ddb75d69733b86fc4c
    just run-wallet vault claim --account-id Public/2RHZhw9h534Zr3eq2RGhQete2Hh667foECzXPmSkGni2 --amount 20000

    just run-wallet account list

# Clean runtime data
clean:
    @echo "🧹 Cleaning run artifacts"
    rm -rf lez/sequencer/service/bedrock_signing_key
    rm -rf lez/sequencer/service/rocksdb
    rm -rf lez/indexer/service/rocksdb
    rm -rf lez/wallet/configs/debug/storage.json
    rm -rf rocksdb
    cd bedrock && docker compose down -v
