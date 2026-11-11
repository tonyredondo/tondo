#!/bin/sh
# Builds the library and the C program that uses it, and runs the program.
# From the directory of the example: sh demo/build.sh
set -e

tondo build --library --output demo/out
cc demo/main.c -Idemo/out -Ldemo/out -ltextkit -Wl,-rpath,"$PWD/demo/out" -o demo/out/demo
demo/out/demo
