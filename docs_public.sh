#!/usr/bin/env sh

cd wasabi-kernel
# cargo clean --doc
cargo tree --depth 1 -e normal --prefix none | cut -d' ' -f1 | xargs printf -- '-p %s\n' | xargs cargo doc --no-deps $@
cd ..
