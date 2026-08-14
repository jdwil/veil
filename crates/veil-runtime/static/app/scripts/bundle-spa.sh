#!/bin/sh
set -e
mkdir -p dist
cp index.html dist/
cp src/spa.js dist/
echo "SPA dist ready"
