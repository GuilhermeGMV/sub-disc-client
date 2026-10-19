#!/usr/bin/env bash
# Usa o cargo do sistema; sem ele, o toolchain local do servidor (../sub-disc/.tools).
set -euo pipefail
cd "$(dirname "$0")/.."
tools="${SUBDISC_TOOLS:-$PWD/../sub-disc/.tools}"
if ! command -v cargo >/dev/null && [ -x "$tools/cargo/bin/cargo" ]; then
  export CARGO_HOME="$tools/cargo" RUSTUP_HOME="$tools/rustup" PATH="$tools/cargo/bin:$PATH"
fi
[ -f .deps/env.sh ] && . .deps/env.sh
exec cargo "$@"
