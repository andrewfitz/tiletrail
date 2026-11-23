# Long-history C snapshot export

Median peak process RSS fell from 130.55 MiB to 104.45 MiB (20.0%) on a 60,000-frame synthetic gallery. All six exports produced 75,290,014 bytes with FNV-1a-64 checksum `866f28e59fe19899`. The returned C buffer owns its JSON and can outlive the analyzer.

| Release build and trial | Result JSON wall seconds | Push and export wall seconds | Peak process RSS, MiB |
| --- | ---: | ---: | ---: |
| Frozen, 1 | 1.9524 | 23.7123 | 125.39 |
| Borrowed, 1 | 1.9164 | 22.6183 | 105.12 |
| Borrowed, 2 | 1.4395 | 20.2081 | 101.03 |
| Frozen, 2 | 1.1721 | 20.2251 | 130.58 |
| Frozen, 3 | 1.1260 | 15.0440 | 130.55 |
| Borrowed, 3 | 0.9776 | 14.3824 | 104.45 |

Median push and export took 20.2251 s before and 20.2081 s after. Median JSON export took 1.1721 s before and 1.4395 s after. Concurrent workstation activity produced substantial wall-time variation, and these runs establish no consistent elapsed-time gain.

## Measurement

The frozen CPU library was built at `d9798c1`. The candidate used the same source with the analyzer snapshot and replay changes in `src/lib.rs`, `src/model.rs` and `src/ffi.rs`. The C client, configuration, compiler and input were identical. The host was an Apple M4 Max running macOS 27.0.1 with rustc 1.99.0. Each row above used a new process. macOS `/usr/bin/time -l` reported maximum resident set size in bytes.

The [C client](../snapshot_history.c) repeatedly submits a single 48 × 32 packed RGB24 buffer at 30 fps. The analyzer retains four regions per frame and the C client requests one snapshot through `tiletrail_analyzer_result_json`. The result timer includes coverage and finalized appearance calculation, JSON serialization and return of the owned allocation. The total timer starts just before the first push. Hashing and releasing the result follow both timers. Peak RSS covers the entire process.

Build the library in each checkout and compile this same client against it. For macOS, run the following from the checkout under measurement, setting `client` to the path of the client above.

```sh
cargo build --locked --release --no-default-features
cc -std=c11 -Wall -Wextra -Werror -O2 -Iinclude "$client" \
  target/release/libtiletrail.a -framework Security -framework CoreFoundation \
  -o target/snapshot_history
/usr/bin/time -l target/snapshot_history
```

Rust callers can stream the same serialized result with `serde_json::to_writer(writer, &analyzer.snapshot(end)?)`. This borrows observations, tracks, events and backend diagnostics. Each snapshot owns its computed coverage ranges and finalized appearances. The existing `finish(end)` method returns an independent copy for callers that keep the result while submitting more frames.
