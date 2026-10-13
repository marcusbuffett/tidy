#!/usr/bin/env bash
# Compare wall-clock time of knip and tidy on a repository.
# usage: bench/compare.sh <repo-dir> [runs] [knip-binary]
set -euo pipefail
repo=${1:?repo dir}; runs=${2:-5}; knip=${3:-$repo/node_modules/.bin/knip}
tidy=$(cd "$(dirname "$0")/.." && pwd)/target/release/tidy
cd "$repo"
best() { sort -n | head -1; }
median() { sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}'; }
t_tidy=(); t_knip=()
for i in $(seq "$runs"); do
  s=$(date +%s.%N); "$tidy" --no-exit-code >/dev/null 2>&1; e=$(date +%s.%N); t_tidy+=("$(awk "BEGIN{print $e-$s}")")
done
if [ -x "$knip" ]; then
  for i in $(seq "$runs"); do
    s=$(date +%s.%N); "$knip" --no-progress >/dev/null 2>&1 || true; e=$(date +%s.%N); t_knip+=("$(awk "BEGIN{print $e-$s}")")
  done
fi
tm=$(printf '%s\n' "${t_tidy[@]}" | median); tb=$(printf '%s\n' "${t_tidy[@]}" | best)
printf 'tidy  median %.3fs  best %.3fs  (%d runs)\n' "$tm" "$tb" "$runs"
if [ ${#t_knip[@]} -gt 0 ]; then
  km=$(printf '%s\n' "${t_knip[@]}" | median); kb=$(printf '%s\n' "${t_knip[@]}" | best)
  printf 'knip  median %.3fs  best %.3fs\n' "$km" "$kb"
  awk "BEGIN{printf \"speedup: %.0fx (median), %.0fx (best)\n\", $km/$tm, $kb/$tb}"
fi
