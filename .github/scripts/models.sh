#!/bin/sh
# Checks the models of the scheduler in testing/models with TLC, every order of
# their steps, given the path of tla2tools.jar. A model in which a task can stay
# in a queue with every worker asleep ends with a deadlock, and this fails.
set -eu
jar=$1
cd testing/models
for config in *.cfg; do
  case "$config" in
    Wake*) model=Wake ;;
    *) model=${config%.cfg} ;;
  esac
  echo "$model with $config"
  java -XX:+UseParallelGC -jar "$jar" -workers auto -config "$config" -metadir "$(mktemp -d)" "$model.tla"
done
