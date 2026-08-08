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

bundled_python_runtime_name=

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
  "$stage/share/completions/zsh" \
  "$stage/share/licenses"

build_wallet_binary() {
  if [[ $target == linux-amd64 ]]; then
    cargo rustc \
      --locked \
      --release \
      --package wallet \
      --bin wallet \
      --features testnet-v0-2 \
      -- \
      -C 'link-arg=-Wl,-rpath,$ORIGIN/../lib'
  else
    cargo build --locked --release --package wallet --bin wallet --features testnet-v0-2
  fi
}

build_wallet_ffi() {
  if [[ $target == linux-amd64 ]]; then
    cargo rustc \
      --locked \
      --release \
      --package wallet-ffi \
      --lib \
      --features testnet-v0-2 \
      -- \
      -C 'link-arg=-Wl,-rpath,$ORIGIN'
  else
    cargo build --locked --release --package wallet-ffi --features testnet-v0-2
  fi
}

python_runtime_name_from_linux_binary() {
  readelf -d "$1" \
    | awk -F'[][]' '/Shared library: \[libpython/ { print $2; exit }'
}

bundle_python_runtime() {
  local wallet="$stage/bin/wallet"
  local wallet_ffi="$stage/lib/libwallet_ffi.$dynamic_extension"
  local python_runtime
  local python_license

  case "$target" in
    linux-amd64)
      python_runtime="$(
        ldd "$wallet" \
          | awk '$1 ~ /^libpython/ && $2 == "=>" && $3 ~ /^\// { print $3; exit }'
      )"
      if [[ -z $python_runtime || ! -f $python_runtime ]]; then
        echo "could not resolve the linked Python runtime for $wallet" >&2
        exit 1
      fi

      bundled_python_runtime_name=$(basename "$python_runtime")
      for artifact in "$wallet" "$wallet_ffi"; do
        if [[ $(python_runtime_name_from_linux_binary "$artifact") != "$bundled_python_runtime_name" ]]; then
          echo "Python runtime mismatch in $artifact" >&2
          exit 1
        fi
      done
      install -m 0644 "$python_runtime" "$stage/lib/$bundled_python_runtime_name"
      ;;
    darwin-arm64)
      python_runtime="$(
        otool -L "$wallet" \
          | awk '$1 ~ /\/Python.framework\/Versions\// && $1 ~ /\/Python$/ { print $1; exit }'
      )"
      if [[ -z $python_runtime || ! -f $python_runtime ]]; then
        echo "could not resolve the linked Python runtime for $wallet" >&2
        exit 1
      fi

      bundled_python_runtime_name=Python
      install -m 0644 "$python_runtime" "$stage/lib/$bundled_python_runtime_name"
      install_name_tool -id @rpath/Python "$stage/lib/$bundled_python_runtime_name"
      for artifact in "$wallet" "$wallet_ffi"; do
        if ! otool -L "$artifact" | awk -v runtime="$python_runtime" '$1 == runtime { found = 1 } END { exit !found }'; then
          echo "Python runtime mismatch in $artifact" >&2
          exit 1
        fi
        install_name_tool -change "$python_runtime" @rpath/Python "$artifact"
      done
      install_name_tool -add_rpath @loader_path/../lib "$wallet"
      install_name_tool -add_rpath @loader_path "$wallet_ffi"
      # Relocating Mach-O load commands invalidates any signature inherited
      # from the host Python framework. Re-sign all modified artifacts so
      # dyld accepts the archive-local runtime on a clean machine.
      for artifact in "$stage/lib/$bundled_python_runtime_name" "$wallet" "$wallet_ffi"; do
        codesign --force --sign - "$artifact"
        codesign --verify --strict "$artifact"
      done
      ;;
  esac

  python_license="$(
    python3 - <<'PY'
from pathlib import Path
import sysconfig

license_path = Path(sysconfig.get_path("stdlib")) / "LICENSE.txt"
if not license_path.is_file():
    raise SystemExit(f"Python license file is unavailable: {license_path}")
print(license_path)
PY
  )"
  install -m 0644 "$python_license" "$stage/share/licenses/Python-PSF-2.0.txt"
}

verify_extracted_wallet_runtime() {
  local archive=$1
  local extracted_root="$temporary/extracted"
  local extracted="$extracted_root/$(basename "$stage")"
  local wallet="$extracted/bin/wallet"
  local wallet_ffi="$extracted/lib/libwallet_ffi.$dynamic_extension"
  local resolved_runtime

  mkdir -p "$extracted_root"
  tar -xzf "$archive" -C "$extracted_root"

  case "$target" in
    linux-amd64)
      local expected_runtime
      expected_runtime="$(readlink -f "$extracted/lib/$bundled_python_runtime_name")"
      [[ -f $expected_runtime ]]
      for artifact in "$wallet" "$wallet_ffi"; do
        resolved_runtime="$(
          ldd "$artifact" \
            | awk -v runtime="$bundled_python_runtime_name" '$1 == runtime && $2 == "=>" { print $3; exit }'
        )"
        if [[ -n $resolved_runtime ]]; then
          resolved_runtime="$(readlink -f "$resolved_runtime")"
        fi
        if [[ $resolved_runtime != "$expected_runtime" ]]; then
          echo "extracted $artifact did not load its bundled Python runtime" >&2
          exit 1
        fi
      done
      env -u LD_LIBRARY_PATH "$wallet" --version
      env -u LD_LIBRARY_PATH python3 - "$wallet_ffi" <<'PY'
import ctypes
import sys

ctypes.CDLL(sys.argv[1])
PY
      ;;
    darwin-arm64)
      [[ -f "$extracted/lib/Python" ]]
      for artifact in "$wallet" "$wallet_ffi"; do
        if ! otool -L "$artifact" | awk '$1 == "@rpath/Python" { found = 1 } END { exit !found }'; then
          echo "extracted $artifact does not reference its bundled Python runtime" >&2
          exit 1
        fi
      done
      if ! otool -l "$wallet" | awk '$1 == "path" && $2 == "@loader_path/../lib" { found = 1 } END { exit !found }'; then
        echo "extracted wallet has no archive-local Python runtime search path" >&2
        exit 1
      fi
      if ! otool -l "$wallet_ffi" | awk '$1 == "path" && $2 == "@loader_path" { found = 1 } END { exit !found }'; then
        echo "extracted wallet FFI has no archive-local Python runtime search path" >&2
        exit 1
      fi
      env -u DYLD_LIBRARY_PATH -u LD_LIBRARY_PATH "$wallet" --version
      env -u DYLD_LIBRARY_PATH -u LD_LIBRARY_PATH python3 - "$wallet_ffi" <<'PY'
import ctypes
import sys

ctypes.CDLL(sys.argv[1])
PY
      ;;
  esac
}

cd "$root"

build_wallet_binary
cargo build --locked --release --package sequencer_service --bin sequencer_service
install -m 0755 "$target_dir/release/sequencer_service" \
  "$temporary/sequencer_service"
cargo build --locked --release --package indexer_service --bin indexer_service
build_wallet_ffi
cargo build --locked --release --package indexer_ffi --features testnet

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

bundle_python_runtime

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
    "$stage/libexec/explorer_service" \
    "$stage/lib/libwallet_ffi.$dynamic_extension" \
    "$stage/lib/$bundled_python_runtime_name"
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

verify_extracted_wallet_runtime "$output_dir/$(basename "$stage").tar.gz"
