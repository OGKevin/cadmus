#!/bin/sh

STAGE="${CADMUS_BENCH_STAGE:?CADMUS_BENCH_STAGE must be set}"

cd "$STAGE" || exit 1
trap 'rm -rf "$STAGE/scratch"' EXIT
rm -rf scratch criterion
mkdir scratch
df -h "$STAGE"

CADMUS_BENCH_DIR="$STAGE/scratch" \
  LD_LIBRARY_PATH="$STAGE/libs" \
  CRITERION_HOME="$STAGE/criterion" \
  ./bench --bench "$@"
