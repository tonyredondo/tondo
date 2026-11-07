#!/bin/sh
# Checks the models of the runtime in testing/models with TLC, every order of
# their steps, given the path of tla2tools.jar and of `tondo`. The same models
# in `std.model`, in testing/models/tests, must go through as many states as
# TLC does: `cargo test` runs them. Then random graphs, formulas of linear
# temporal logic and fairness, which std.model, TLC and a brute force over the
# runs must judge the same; the third argument is the seed, and the fourth how
# many there are.
set -eu
jar=$(realpath "$1")
tondo=$(realpath "$2")
seed=${3:-1}
count=${4:-200}
cd testing/models
for config in *.cfg; do
  case "$config" in
    Wake*) model=Wake ;;
    *) model=${config%.cfg} ;;
  esac
  echo "$model with $config"
  out=$(mktemp)
  java -XX:+UseParallelGC -jar "$jar" -workers auto -config "$config" -metadir "$(mktemp -d)" "$model.tla" > "$out" 2>&1 \
    || { cat "$out"; exit 1; }
  cat "$out"
  states=$(sed -n 's/.* \([0-9][0-9]*\) distinct states found.*/\1/p' "$out" | tail -n 1)
  test=tests/$(echo "$model" | tr '[:upper:]' '[:lower:]').to
  grep -q ".states == $states)" "$test" || {
    echo "$test does not say it goes through the $states states TLC does with $config"
    exit 1
  }
done

ltl=$(mktemp -d)
echo "linear temporal logic: $count cases of seed $seed"
python3 ltl/gen.py "$seed" "$count" "$ltl"
"$tondo" run "$ltl/model.to" > "$ltl/tondo.txt" 2> "$ltl/errors.txt" || { cat "$ltl/errors.txt"; exit 1; }
python3 ltl/brute.py "$ltl/cases.json" > "$ltl/brute.txt"
if ! cmp -s "$ltl/tondo.txt" "$ltl/brute.txt"; then
  echo "std.model and the brute force differ, as std.model < > the brute force, by case from 1:"
  diff "$ltl/tondo.txt" "$ltl/brute.txt" || true
  exit 1
fi
i=0
while read -r said; do
  spec=$ltl/tla/G$i
  if [ -f "$spec.tla" ]; then
    (cd "$ltl/tla" && java -XX:+UseParallelGC -jar "$jar" -workers 1 -config "G$i.cfg" -metadir "$(mktemp -d)" "G$i.tla") \
      > "$spec.out" 2>&1 || true
    if grep -q "No error has been found" "$spec.out"; then
      tlc=holds
    elif grep -q -e "Temporal properties were violated" -e "is violated" "$spec.out"; then
      tlc=fails
    else
      cat "$spec.out"
      exit 1
    fi
    if [ "$tlc" != "$said" ]; then
      echo "case $i: std.model says the formula $said, TLC that it $tlc"
      cat "$spec.tla"
      exit 1
    fi
  fi
  i=$((i + 1))
done < "$ltl/tondo.txt"
echo "std.model, TLC and the brute force agree on $i cases"
