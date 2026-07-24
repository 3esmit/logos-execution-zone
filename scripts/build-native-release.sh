#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 VERSION TARGET COMMIT OUTPUT_DIR" >&2
  exit 2
fi

version=$1
target=$2
commit=$3
output_dir=$4
root=$(git rev-parse --show-toplevel)

python3 "$root/scripts/native_release.py" validate \
  --version "$version" \
  --target "$target" \
  --commit "$commit"

case "$target" in
  linux-amd64)
    [[ $(uname -s) == Linux ]]
    [[ $(uname -m) == x86_64 ]]
    dynamic_extension=so
    ;;
  darwin-arm64)
    [[ $(uname -s) == Darwin ]]
    [[ $(uname -m) == arm64 ]]
    dynamic_extension=dylib
    ;;
  *)
    echo "unsupported native release target: $target" >&2
    exit 2
    ;;
esac

target_dir=${CARGO_TARGET_DIR:-"$root/target"}
if [[ $target_dir != /* ]]; then
  target_dir="$root/$target_dir"
fi

temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
stage="$temporary/logos-execution-zone-$version-$target"
mkdir -p \
  "$stage/bin" \
  "$stage/lib" \
  "$stage/libexec" \
  "$stage/include" \
  "$stage/share/explorer" \
  "$stage/share/config-examples" \
  "$stage/share/completions/bash" \
  "$stage/share/completions/zsh"

cd "$root"

cargo build --locked --release --package wallet --bin wallet
cargo build --locked --release --package sequencer_service --bin sequencer_service
install -m 0755 "$target_dir/release/sequencer_service" \
  "$temporary/sequencer_service"
cargo build --locked --release --package indexer_service --bin indexer_service
cargo build --locked --release --package wallet-ffi
cargo build --locked --release --package indexer_ffi

cargo build \
  --locked \
  --release \
  --package sequencer_service \
  --bin sequencer_service \
  --features standalone
install -m 0755 "$target_dir/release/sequencer_service" \
  "$temporary/sequencer_service-standalone"

cargo leptos build --release -vv

install -m 0755 "$target_dir/release/wallet" "$stage/bin/wallet"
install -m 0755 "$temporary/sequencer_service" "$stage/bin/sequencer_service"
install -m 0755 "$temporary/sequencer_service-standalone" \
  "$stage/bin/sequencer_service-standalone"
install -m 0755 "$target_dir/release/indexer_service" "$stage/bin/indexer_service"
install -m 0755 "$target_dir/release/explorer_service" \
  "$stage/libexec/explorer_service"
install -m 0755 "$root/scripts/explorer-service-launcher" \
  "$stage/bin/explorer_service"
install -m 0755 "$(command -v r0vm)" "$stage/bin/r0vm"

install -m 0644 "$target_dir/release/libwallet_ffi.$dynamic_extension" "$stage/lib/"
install -m 0644 "$target_dir/release/libindexer_ffi.$dynamic_extension" "$stage/lib/"

install -m 0644 "$root/lez/wallet-ffi/wallet_ffi.h" "$stage/include/"
install -m 0644 "$root/lez/indexer/ffi/indexer_ffi.h" "$stage/include/"
install -m 0644 "$root/lez/wallet/configs/debug/wallet_config.json" \
  "$stage/share/config-examples/"
install -m 0644 "$root/lez/sequencer/service/configs/debug/sequencer_config.json" \
  "$stage/share/config-examples/"
install -m 0644 "$root/lez/indexer/service/configs/debug/indexer_config.json" \
  "$stage/share/config-examples/"
install -m 0644 "$root/completions/bash/wallet" \
  "$stage/share/completions/bash/wallet"
install -m 0644 "$root/completions/zsh/_wallet" \
  "$stage/share/completions/zsh/_wallet"
install -m 0644 "$root/lez/explorer_service/Cargo.toml" \
  "$stage/share/explorer/Cargo.toml"
cp -R "$target_dir/site" "$stage/share/explorer/site"
install -m 0644 "$root/docs/native-releases.md" "$stage/README.md"
install -m 0644 "$root/LICENSE" "$stage/LICENSE"

export PATH="$stage/bin:$PATH"
export RISC0_SERVER_PATH="$stage/bin/r0vm"
"$stage/bin/wallet" --version
"$stage/bin/sequencer_service" --version
"$stage/bin/sequencer_service-standalone" --version
"$stage/bin/indexer_service" --version
"$stage/bin/explorer_service" --version
"$stage/bin/r0vm" --version

python3 - \
  "$stage/lib/libwallet_ffi.$dynamic_extension" \
  "$stage/lib/libindexer_ffi.$dynamic_extension" <<'PY'
import ctypes
import sys

for library in sys.argv[1:]:
    ctypes.CDLL(library)
    print(f"loaded {library}")
PY

runtime_report="$stage/share/runtime-dependencies.txt"
{
  printf 'Target: %s\n' "$target"
  printf 'Source commit: %s\n' "$commit"
  for program in \
    "$stage/bin/wallet" \
    "$stage/bin/sequencer_service" \
    "$stage/bin/sequencer_service-standalone" \
    "$stage/bin/indexer_service" \
    "$stage/libexec/explorer_service"
  do
    printf '\n%s\n' "${program#"$stage/"}"
    if [[ $target == linux-amd64 ]]; then
      ldd "$program"
    else
      otool -L "$program"
    fi
  done
} > "$runtime_report"

python3 "$root/scripts/native_release.py" pack \
  --stage "$stage" \
  --output-dir "$output_dir" \
  --version "$version" \
  --target "$target" \
  --commit "$commit"
