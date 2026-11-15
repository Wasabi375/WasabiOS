#!/usr/bin/env sh

if ! [ -z ${WOSBUILD_NOFS+x} ]; then
    echo "Disabled FS access for build script"
    NO_DEFAULT_FEATURES="--no-default-features"
fi

cargo run $NO_DEFAULT_FEATURES --release --package builder -- $@
