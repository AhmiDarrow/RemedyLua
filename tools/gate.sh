#!/usr/bin/env bash
# The gate: runs the steps a change's tier needs, timing tests apart from the rest.
#
#   tools/gate.sh <tier> [args]
#
# Tiers (docs/testing.md, "The gate", says which change takes which):
#   docs <base>       the change touches only docs: checks that, runs no test
#   tests <name>...   tests/ only (not tests/common): the named tests, then D
#   helper            tests/common or a helper shared across modules: D T
#   stdlib            a library leaf (stdlib/* but mod.rs, fmt.rs): D T L
#   front             lexer, parser, compiler, ast, check, diag: D O T L W
#   core              heap, gctrace, table, vm, fast, value, number, bytecode, lib.rs,
#                     stdlib/mod.rs, shardmap, or anything else: D Dn T Tn O L S W
#
# Steps:
#   D   every test but the [timing] ones, in parallel
#   Dn  D with REMEDYLUA_TEST_NO_FAST_LOOP=1
#   T   the [timing] tests, one at a time
#   Tn  the [env] tests (the part of [timing] that follows the variable), with it set
#   O   the [oracle] tests, with --features front-oracle
#   L   the unit tests (--lib)
#   S   the release collector stress over the corpus
#   W   the wasm smoke test
# Lists: crates/remedy-lua/tests/timing.list, kept true by the test_lists guard (in D).
# A [known] test (tied to an open finding, gate rule 5) runs and is reported where it
# runs, but a step whose only failures are [known] ones does not fail. On Windows the
# [known-windows] tests count as [known] too.
#
# Run it on a quiet machine: T and Tn measure time. Everything is built before the
# first step, and the script reports other test or compiler processes at each step.
# LUA_SRC is used if set, else the lua-src crate's Lua 5.4.7 sources are looked for
# in the cargo registry. Logs go to $GATE_LOGS (default target/gate/<time>).
# Exit status: 0 if every step passed.
set -u
cd "$(dirname "$0")/.." || exit 2

LIST=crates/remedy-lua/tests/timing.list
TIER=${1:-}
shift || true

section() { awk -v s="[$1]" '$0 == s { on = 1; next } /^\[/ { on = 0 } on && NF && $0 !~ /^#/' "$LIST"; }

if [ -z "${LUA_SRC:-}" ]; then
  for d in "$HOME"/.cargo/registry/src/*/lua-src-*/lua-5.4.7 "$HOME"/.cargo/registry/src/*/lua-src-*/lua-5.4.7/src; do
    [ -f "$d/lua.h" ] && LUA_SRC=$d
  done
fi
[ -n "${LUA_SRC:-}" ] && [ -f "$LUA_SRC/lua.h" ] || { echo "gate: set LUA_SRC to Lua 5.4.7's src (run cargo fetch first)" >&2; exit 2; }
export LUA_SRC

LOGS=${GATE_LOGS:-target/gate/$(date +%Y%m%d-%H%M%S)}
mkdir -p "$LOGS"
mapfile -t TIMING < <(section timing)
mapfile -t ENVD < <(section env)
mapfile -t ORACLE < <(section oracle)
mapfile -t KNOWN < <(section known)
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) mapfile -t -O "${#KNOWN[@]}" KNOWN < <(section known-windows) ;; esac
known_finding() { local k; for k in "${KNOWN[@]}"; do [ "${k%% *}" = "$1" ] && { echo "${k#* }"; return 0; }; done; return 1; }
SKIP=()
for t in "${TIMING[@]}"; do SKIP+=(--skip "$t"); done

ALL=(cargo test -p remedy-lua --test all)
ORA=(cargo test -p remedy-lua --features front-oracle --test all)
LIB=(cargo test -p remedy-lua --lib)
REL=(cargo test --release -p remedy-lua --test all)

others() { pgrep -fa 'rustc|cargo|deps/all-' 2>/dev/null | grep -v -e defunct -e "$$" -e 'tools/gate.sh' | wc -l; }

FAILED=()
step() {
  local name=$1; shift
  echo "== $name  $(uptime 2>/dev/null | sed 's/^ *//')  other processes: $(others)"
  "$@" > "$LOGS/$name.log" 2>&1
  local rc=$?
  grep -E '^test result|^error|FAILED|wasm smoke' "$LOGS/$name.log" | sed 's/^/   /' | head -12
  local k res
  for k in "${KNOWN[@]}"; do
    res=$(grep -oE "^test ${k%% *} \.\.\. [a-zA-Z]+" "$LOGS/$name.log" | awk '{print $NF}')
    [ -n "$res" ] && echo "   known (${k#* }): ${k%% *} $res"
  done
  if [ $rc -ne 0 ]; then
    # Only [known] tests failed: reported above, not counted. Anything else (another
    # test, a build error, no test lines at all) fails the step.
    local failed unknown=0 t
    failed=$(grep -oE '^test \S+ \.\.\. FAILED' "$LOGS/$name.log" | awk '{print $2}')
    [ -z "$failed" ] && unknown=1
    for t in $failed; do known_finding "$t" > /dev/null || unknown=1; done
    if [ $unknown -eq 0 ]; then
      echo "   $name: only [known] tests failed (gate rule 5): not counted"
    else
      FAILED+=("$name"); echo "   $name FAILED (exit $rc, log $LOGS/$name.log)"
    fi
  fi
}
D()  { step D "${ALL[@]}" -- --exact "${SKIP[@]}"; }
Dn() { step Dn env REMEDYLUA_TEST_NO_FAST_LOOP=1 "${ALL[@]}" -- --exact "${SKIP[@]}"; }
T()  { step T "${ALL[@]}" -- --exact --test-threads=1 "${TIMING[@]}"; }
Tn() { step Tn env REMEDYLUA_TEST_NO_FAST_LOOP=1 "${ALL[@]}" -- --exact --test-threads=1 "${ENVD[@]}"; }
O()  { step O "${ORA[@]}" -- --exact "${ORACLE[@]}"; }
L()  { step L "${LIB[@]}"; }
S()  { step S env REMEDYLUA_S3_SECS=1 "${REL[@]}" sweep3_stab::gc_stress_over_the_corpus; }
W()  { step W node tools/wasm-smoke/smoke.mjs; }

build() {
  echo "== build ($*)"
  local b
  for b in "$@"; do
    case $b in
      all) "${ALL[@]}" --no-run -q ;;
      oracle) "${ORA[@]}" --no-run -q ;;
      lib) "${LIB[@]}" --no-run -q ;;
      release) "${REL[@]}" --no-run -q ;;
      wasm) rustup target add wasm32-unknown-unknown > /dev/null 2>&1 ;;
    esac > "$LOGS/build-$b.log" 2>&1 || { echo "gate: build $b failed, see $LOGS/build-$b.log" >&2; exit 1; }
  done
}

echo "gate: tier ${TIER:-?}, HEAD $(git rev-parse --short HEAD), tree $(git rev-parse HEAD^{tree}), logs $LOGS"
case $TIER in
  docs)
    base=${1:?"gate docs <base>: the commit the change is on"}
    bad=$(git diff --name-only "$base" HEAD | grep -v -E '\.md$' || true)
    if [ -n "$bad" ]; then echo "gate: not docs only, these need a higher tier:"; echo "$bad"; exit 1; fi
    echo "gate: docs only ($(git diff --name-only "$base" HEAD | wc -l) files), no tests to run"
    exit 0 ;;
  tests)
    [ $# -gt 0 ] || { echo "gate tests <name>...: the changed tests, as --list prints them" >&2; exit 2; }
    build all
    serial=()
    for n in "$@"; do printf '%s\n' "${TIMING[@]}" | grep -qx -- "$n" && serial=(--test-threads=1); done
    step changed "${ALL[@]}" -- --exact "${serial[@]}" "$@"
    D ;;
  helper) build all; D; T ;;
  stdlib) build all lib; D; T; L ;;
  front) build all oracle lib wasm; D; O; T; L; W ;;
  core) build all oracle lib release wasm; D; Dn; T; Tn; O; L; S; W ;;
  *) sed -n '2,32p' "$0"; exit 2 ;;
esac

if [ ${#FAILED[@]} -eq 0 ]; then echo "gate: PASS"; else echo "gate: FAIL in ${FAILED[*]}"; exit 1; fi
