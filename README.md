# TileTrail

TileTrail tracks participant tiles across layout changes in video recordings. Supply decoded frames on their original presentation clock. The result describes visible regions, changes in geometry and camera state, possible identities, and intervals of uncertain coverage.

## Analyze frames from Rust

```rust
use tiletrail::{Analyzer, AnalyzerConfig, Frame, PixelFormat, Timestamp};

let start = Timestamp::new(9000, 1, 90000);
let mut analyzer = Analyzer::new(AnalyzerConfig::default(), start)?;
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

Enable the `gpu` Cargo feature for GPU-first analysis. The default analyzer configuration prefers Metal on Apple Silicon and recovers on the CPU if the GPU becomes unavailable. On other platforms automatic mode uses the CPU. A build without the feature omits wgpu. Both paths feed the same identity tracker. Call `analyzer.diagnostics()` to inspect the selected backend and transitions.

## Interchange

Track IDs belong to one analysis. An editor can link a durable person ID to accepted appearance intervals and preserve corrections across project saves. A guest may occupy more than one region. Ambiguous associations and the confidence attached to them are available to the consumer. Review the actual regions and coverage before using an inferred association for privacy editing.

The serialized result carries a schema version distinct from the crate version and the C ABI version. [JSON result schema](schema/analysis-v1.json)

The [two-person example](schema/examples/two-up.json) contains the first two frames of the generated interview at 10 fps. The observations identify two visible regions at source times 0.0 and 0.1 seconds. Each region has an appearance ID and a participant ID. The scan ends at 0.2 seconds, so an editor can query either frame using the same half-open time range.

## Build and test

```sh
cargo test --locked
cargo test --locked --features gpu
cargo build --locked --release --features gpu
```

For linked C, Swift and Python examples, see the [C consumer guide](examples/c/README.md). The [gallery benchmarks](benchmarks/README.md) measure geometry, identity, event timing and process memory. The [real-recording validation set](validation/real/README.md) includes attributed video excerpts and the procedure for scoring them.

The library is distributed under the [MIT license](LICENSE). The real recording excerpts carry their own licenses, documented with the clips.

The [0.1.0 verification report](validation/RELEASE.md) records the tested platforms, package checksums, held-out recording results and measured performance.
