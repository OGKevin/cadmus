#!/usr/bin/env bash
set -euo pipefail

libdir="$(llvm-config --libdir 2>/dev/null || true)"
if [[ -z $libdir ]]; then
  candidate="$(find /usr/lib \( -name 'libclang.so*' -o -name 'libclang-*.so*' \) 2>/dev/null | head -1)"
  if [[ -n $candidate ]]; then
    libdir="$(dirname "$candidate")"
  fi
fi

if [[ -z $libdir || ! -d $libdir ]]; then
  echo "Unable to locate libclang; install llvm/clang development packages or set LIBCLANG_PATH." >&2
  exit 1
fi

echo "$libdir"
