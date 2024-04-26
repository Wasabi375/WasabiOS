#!/usr/bin/env sh

cargodoc() {
 cargo tree --depth 1 -e normal --prefix none | cut -d' ' -f1 | xargs printf -- '-p %s\n' | xargs cargo doc --lib --features test --no-deps --document-private-items
}

cd wasabi-kernel
# cargo clean --doc
cargodoc
cd ..
cd wasabi-test
cargodoc
cd ..

if [ "$1" = "open" ] || [ "$1" = "--open" ] ; then
    if [ -z "$2"  ]; then
        xdg-open target/doc/wasabi_kernel/index.html
    else 
        xdg-open "target/doc/$2/index.html"
    fi
fi

