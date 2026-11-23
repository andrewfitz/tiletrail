#!/usr/bin/env bash
set -euo pipefail

root=$(pwd)
[[ $(uname -s) = Darwin && $(uname -m) = arm64 ]]
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/tiletrail-macos-aarch64"/{include,lib}
prefix=$stage/tiletrail-macos-aarch64
cp include/{tiletrail.h,module.modulemap} "$prefix/include/"
cp target/release/{libtiletrail.a,libtiletrail.dylib} "$prefix/lib/"
cp LICENSE "$prefix/"
install_name_tool -id '@rpath/libtiletrail.dylib' "$prefix/lib/libtiletrail.dylib"
codesign --force --sign - "$prefix/lib/libtiletrail.dylib"
tar -czf target/tiletrail-macos-aarch64.tar.gz -C "$stage" tiletrail-macos-aarch64

install="$stage/installed"
mkdir -p "$install"
tar -xzf target/tiletrail-macos-aarch64.tar.gz -C "$install"
prefix=$install/tiletrail-macos-aarch64
cp examples/c/analyze.c examples/swift/analyze.swift "$install/"
(
  cd "$install"
  cc -std=c11 -Wall -Wextra -Werror -I"$prefix/include" analyze.c \
    -L"$prefix/lib" -ltiletrail -Wl,-rpath,"$prefix/lib" -o analyze-c
  swiftc -I "$prefix/include" analyze.swift "$prefix/lib/libtiletrail.a" \
    -framework Foundation -framework QuartzCore -framework Metal -lSystem \
    -framework CoreGraphics -framework CoreFoundation -lobjc -liconv -lSystem -lc -lm \
    -o analyze-swift
  fixture=$root/target/ffi-gallery/gallery.rgb
  ./analyze-c "$fixture" 960 540 0 1 10 > c.json
  ./analyze-swift "$fixture" 960 540 0 1 10 > swift.json
  python3 "$root/.github/verify_consumers.py" "$root/target/ffi-gallery/rust.json" c.json swift.json
)
