# Consume TileTrail through the C ABI

[The public header](../../include/tiletrail.h) defines ABI version 1 and the
ownership rules. Call `tiletrail_abi_version()` to check the loaded library.
The buffer returned by `tiletrail_analyzer_result_json()` contains the
[versioned analysis document](../../schema/analysis-v1.json). Use the same
header to query scene markers from a saved result.

Build the library and generate the redistributable gallery input from the
repository root.

```sh
cargo build --locked --release
python3 benchmarks/make_gallery.py target/fixture --case gallery
```

On macOS or Linux, link the sample against the shared library.

```sh
cc -std=c11 -Iinclude examples/c/analyze.c -Ltarget/release -ltiletrail \
  -Wl,-rpath,"$PWD/target/release" -o target/analyze
DYLD_LIBRARY_PATH="$PWD/target/release" \
  target/analyze target/fixture/gallery.rgb 960 540 0 1 10 > target/analysis.json
```

On Windows, use an x64 Visual Studio developer prompt.

```bat
cl /nologo /std:c11 /D_CRT_SECURE_NO_WARNINGS /Iinclude examples\c\analyze.c target\release\tiletrail.dll.lib /Fetarget\analyze.exe
set "PATH=%CD%\target\release;%PATH%"
target\analyze.exe target\fixture\gallery.rgb 960 540 0 1 10 > target\analysis.json
```

The [Python sample](../python/analyze.py) uses the same shared library via
`ctypes`. Pass `libtiletrail.dylib` on macOS, `libtiletrail.so` on Linux,
or `tiletrail.dll` on Windows.

```sh
python3 examples/python/analyze.py target/release/libtiletrail.dylib \
  target/fixture/gallery.rgb 960 540 0 1 10
```

The Apple Silicon distribution contains the header and Swift module map in
`include/` and the static and shared libraries in `lib/`. The
[packaging smoke test](../../.github/package-macos.sh) shows how to link a
clean C project and an [independent Swift consumer](../swift/analyze.swift)
against the extracted files.
