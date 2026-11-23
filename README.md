# TileTrail

TileTrail tracks participant tiles across layout changes in video recordings. Supply decoded frames on their original presentation clock. The result describes visible regions, changes in geometry and camera state, possible identities, and intervals of uncertain coverage.

## Analyze frames from Rust

```rust
use tiletrail::{Analyzer, AnalyzerConfig, BackendMode, Frame, PixelFormat, Timestamp};

let start = Timestamp::new(9000, 1, 90000);
let config = AnalyzerConfig { mode: BackendMode::CpuOnly, ..Default::default() };
let mut analyzer = Analyzer::new(config, start)?;
let pixels = vec![96_u8; 320 * 180];
analyzer.push(Frame {
    data: &pixels, width: 320, height: 180, stride: 320,
    format: PixelFormat::Gray8, timestamp: start, display: Default::default(),
})?;
let end = Timestamp::new(12000, 1, 90000);
let result = analyzer.finish(end)?;
assert_eq!(result.schema_version, 1);
# Ok::<(), tiletrail::AnalysisError>(())
```

The frame buffer is borrowed for the call. Supply actual presentation timestamps, including the media origin, and the exclusive end of the last frame. Rectangle coordinates run from zero to one in display orientation, with the origin in the upper left. `visible_at` returns a sampled observation. Check `coverage_at` to distinguish observed, inferred, unknown and incomplete time.

The default crate build uses the Rust CPU backend. Enable the `gpu` Cargo feature to include the optional wgpu backend. Use automatic mode for backend selection and fallback, or request a specific mode for repeatable measurements. Both paths feed the same identity tracker.

## Interchange

Track IDs belong to one analysis. An editor can link a durable person ID to accepted appearance intervals and preserve corrections across project saves. A guest may occupy more than one region. Ambiguous associations and the confidence attached to them are available to the consumer. Review the actual regions and coverage before using an inferred association for privacy editing.

The serialized result carries a schema version distinct from the crate version and the C ABI version. [JSON result schema](schema/analysis-v1.json)

## Build and test

```sh
cargo test --locked
cargo test --locked --features gpu
cargo build --locked --release --features gpu
```

For linked C, Swift and Python examples, see the [C consumer guide](examples/c/README.md). The [gallery benchmarks](benchmarks/README.md) measure geometry, identity, event timing and process memory. The [real-recording validation set](validation/real/README.md) includes attributed video excerpts and the procedure for scoring them.

The library is distributed under the [MIT license](LICENSE). The real recording excerpts carry their own licenses, documented with the clips.
