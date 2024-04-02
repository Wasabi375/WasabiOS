#!/usr/bin/env sh

cargodoc() {
 cargo tree --depth 1 -e normal --prefix none | cut -d' ' -f1 | xargs printf -- '-p %s\n' | xargs cargo doc --lib --features test --no-deps --document-private-items  $@
}

cd wasabi-kernel
# cargo clean --doc
cargodoc
cd ..
cd wasabi-test
cargodoc
cd ..
