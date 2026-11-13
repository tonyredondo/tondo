#!/bin/sh
# Builds the program and its two plugins, and renders sales.csv with each
# renderer. From the directory of the example: sh build.sh
set -e

mkdir -p out/plugins
(cd host && tondo build --output ../out/tabulate)
(cd markdown && tondo build --library --output ../out/markdown)
cp out/markdown/libmarkdown.* out/plugins/
rm -f out/plugins/libmarkdown.a
(cd json && tondo build --output ../out/plugins/json)

for renderer in plain markdown json; do
    echo "--- $renderer"
    out/tabulate sales.csv $renderer out/plugins
done
