#!/usr/bin/env bash
#
# Compare the walk in the working tree against the walk on another ref, for
# both speed and peak memory, on real directories.
#
# **Why interleaved and not sequential.** Running all of A then all of B
# measures the machine warming up as much as it measures the code: on 9
# September 2026 a sequential run reported a 12% regression that an interleaved
# run turned into a small improvement. Both binaries see the same thermal state
# and the same page cache only if they alternate, so this runs A, B, B, A, A, B
# ... and takes the median of each.
#
# **Why the median and not the mean.** One scheduling hiccup moves a mean and
# does not move a median, and there is no way to quiet this machine enough for
# the mean to be meaningful.
#
# **Reading the result.** If the two distributions overlap, the honest report
# is "the same", not the difference between the medians. The min and max are
# printed for exactly that judgement — see B5 in TODO.md, where they did not
# overlap and the claim was made on that basis.
#
# Usage:
#   scripts/bench-walk.sh [corpus ...]
#   BASE_REF=main ROUNDS=9 scripts/bench-walk.sh ~/github /Applications
set -euo pipefail

BASE_REF="${BASE_REF:-main}"
ROUNDS="${ROUNDS:-9}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
WORKTREE="${WORKTREE:-${TMPDIR:-/tmp}/spacetrace-bench-base}"

if [ "$#" -gt 0 ]; then
    CORPORA=("$@")
else
    CORPORA=("$HOME/github" "/Applications")
fi

# The baseline is built from a detached worktree rather than from a stash, so
# that an interrupted run cannot leave the working tree holding someone else's
# commit.
if [ ! -d "$WORKTREE" ]; then
    echo "==> checking out $BASE_REF into $WORKTREE"
    git -C "$REPO" worktree add --detach "$WORKTREE" "$BASE_REF" >/dev/null
else
    echo "==> reusing $WORKTREE (remove it to re-checkout)"
fi

echo "==> building baseline ($BASE_REF)"
cargo build --release --manifest-path "$WORKTREE/Cargo.toml" \
    -p spacetrace-scan-core --example memprobe 2>&1 | tail -2
echo "==> building candidate (working tree)"
cargo build --release --manifest-path "$REPO/Cargo.toml" \
    -p spacetrace-scan-core --example memprobe 2>&1 | tail -2

BASE_BIN="$WORKTREE/target/release/examples/memprobe"
CAND_BIN="$REPO/target/release/examples/memprobe"

for bin in "$BASE_BIN" "$CAND_BIN"; do
    if [ ! -x "$bin" ]; then
        echo "missing $bin — does $BASE_REF have examples/memprobe.rs?" >&2
        exit 1
    fi
done

# `RESULT <entries> <duration_ms> <peak_bytes>`; see examples/memprobe.rs.
run_one() {
    "$1" scan "$2" | awk '/^RESULT/ { print $3, $4 }'
}

summarise() {
    # median, min, max of the numbers on stdin, one per line
    sort -n | awk '
        { v[NR] = $1 }
        END {
            if (NR == 0) { print "n/a"; exit }
            m = (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2
            printf "%.0f (min %.0f, max %.0f)", m, v[1], v[NR]
        }'
}

for corpus in "${CORPORA[@]}"; do
    if [ ! -d "$corpus" ]; then
        echo "==> skipping $corpus (not a directory)"
        continue
    fi
    echo
    echo "==> $corpus, $ROUNDS interleaved rounds"

    base_ms=""; base_rss=""; cand_ms=""; cand_rss=""
    for round in $(seq 1 "$ROUNDS"); do
        # Order flips every round, so neither binary always runs on a colder
        # cache than the other.
        if [ $((round % 2)) -eq 1 ]; then
            order="base cand"
        else
            order="cand base"
        fi
        for which in $order; do
            if [ "$which" = "base" ]; then
                read -r ms rss <<<"$(run_one "$BASE_BIN" "$corpus")"
                base_ms="$base_ms$ms
"
                base_rss="$base_rss$rss
"
            else
                read -r ms rss <<<"$(run_one "$CAND_BIN" "$corpus")"
                cand_ms="$cand_ms$ms
"
                cand_rss="$cand_rss$rss
"
            fi
        done
        printf '  round %s done\n' "$round"
    done

    echo "  duration ms  base  $(printf '%s' "$base_ms" | grep -v '^$' | summarise)"
    echo "  duration ms  cand  $(printf '%s' "$cand_ms" | grep -v '^$' | summarise)"
    echo "  peak bytes   base  $(printf '%s' "$base_rss" | grep -v '^$' | summarise)"
    echo "  peak bytes   cand  $(printf '%s' "$cand_rss" | grep -v '^$' | summarise)"
done

echo
echo "remove the baseline worktree with: git -C $REPO worktree remove $WORKTREE"
