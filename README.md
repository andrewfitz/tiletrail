# TileTrail

TileTrail detects and tracks rectangular video feeds in a single composited recording. It can analyze a camera multiview, follow tiles through a changing layout, or interpret a video call with participants and camera-off placeholders. Supply decoded frames on their original presentation clock. The result describes visible regions, changes in geometry and camera state, possible identities, and intervals of uncertain coverage.

## Analyze frames from Rust

From a Rust application next to this checkout, run `cargo add tiletrail --path ../tiletrail --features gpu`. A consumer can also use a local directory extracted from the packaged `.crate` archive. Supply decoded frames through the Rust API below.

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

Choose the analysis mode for the recording. For automatic detection of a camera multiview:

```rust
use tiletrail::{Analyzer, AnalyzerConfig, FeedLayout, Timestamp};

let start = Timestamp::new(0, 1, 30);
let analyzer = Analyzer::with_layout(AnalyzerConfig::default(), start, FeedLayout::Automatic)?;
# Ok::<(), tiletrail::AnalysisError>(())
```

- **Automatic** infers rectangular feed boundaries from the picture. Use it for changing layouts, including asymmetric arrangements and bordered picture-in-picture. An inset has its own crop. The base region includes the inset as recorded. Each region has display-space bounds and appearance-based identity evidence. Inspect uncertain matches after a cut or rearrangement.
- **Grid** takes a known number of equal columns and rows, for example `FeedLayout::Grid { columns: 2, rows: 2 }`. Each of up to 64 cells keeps its row-major display identity through black frames, duplicated images and cuts. A camera routed into a different cell takes that cell’s identity.
- **Meeting** adds participant-specific interpretation of galleries, screen shares and camera-off states. `Analyzer::new` continues to use this mode.

Rust consumers can use `feed_tracks()`, `feed_appearances()`, and the `feed_id()` on a region. The version 1 interchange format uses the existing `participant_id` field for a feed identity. In C, call `tiletrail_analyzer_create_feeds` with zero columns and rows for automatic detection or positive dimensions for a grid.

The frame buffer is borrowed for the call. Supply actual presentation timestamps, including the media origin, and the exclusive end of the last frame. Rectangle coordinates run from zero to one in display orientation, with the origin in the upper left. `visible_at` returns a sampled observation. Check `coverage_at` to distinguish observed, inferred, unknown and incomplete time.

Enable the `gpu` Cargo feature to make the Metal backend available. Automatic mode measures on the CPU: a measurement reads a fixed grid of point samples, which the CPU does faster than a GPU upload and readback at every frame size, with identical values. Request the GPU explicitly with `BackendMode::GpuRequired`. A build without the feature omits wgpu. Both paths feed the same identity tracker. Call `analyzer.diagnostics()` to inspect the selected backend and transitions.

## Interchange

Track IDs belong to one analysis. An editor can link a durable person ID to accepted appearance intervals and preserve corrections across project saves. A guest may occupy more than one region. Ambiguous associations and the confidence attached to them are available to the consumer. Review the actual regions and coverage before using an inferred association for privacy editing.

If a consumer saves the raw result, save the selected layout alongside it to retain the identity convention used for that scan.

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

The [camera multiview validation](validation/feeds/README.md) measures automatic detection on moving footage, including a borderless quad, a camera swap and a compressed inset.

The [0.1.0 verification report](validation/RELEASE.md) records the tested platforms, package checksums, held-out recording results and measured performance.

### Reporting progress

After pushing a frame, call `analyzer.progress(Some(exclusive_end))?` for its committed frame count, latest source timestamp, state and whole percentage. Supply the end in the recording’s source clock. With an unknown duration, pass `None` and use the count or timestamp. Progress queries take constant time and can be polled between pushes. A successful finished range reports 100 through `result.progress()?`. A frame that requires replay leaves progress at the last committed frame and reports `NeedsReplay`. Supply an end at or after the pending timestamp.

C consumers can poll `tiletrail_analyzer_progress` when `TILETRAIL_HAS_PROGRESS` is defined. Pass a null end for unknown duration (`percent == -1`). The live analyzer stays available for further pushes after a snapshot. A consumer finishing a finite input can display 100 after receiving its successful snapshot with no resume point.

### Skipping unchanged stretches

A caller that can seek, such as one decoding only keyframes, can offer a later frame with `Analyzer::push_coarse` (`tiletrail_analyzer_push_coarse` in C, present when `TILETRAIL_HAS_COARSE_PUSH` is defined). TileTrail commits the frame only when every region continues from the last committed frame: the same appearances and identity state, no event, and bounds within the tolerance of a move or resize. The skipped interval is then reported as inferred coverage, also in every-frame mode.

Otherwise the call returns `Refine` and leaves the analyzer exactly as it was. Push the skipped frames and then that frame with `push`. The result is the same as if the coarse call had never been made. A change that appears and fully reverts between two committed frames is not seen, so keep the skipped interval short.

### Preparing frames on several threads

`Analyzer::prepare` measures and detects a frame without changing the analyzer, so a caller can prepare many frames at once and then commit them in presentation order with `push_prepared` or `push_coarse_prepared`. The result equals pushing the frames themselves. Preparation measures on the CPU. In C, use `tiletrail_analyzer_prepare`, `tiletrail_analyzer_push_prepared` and `tiletrail_prepared_free` when `TILETRAIL_HAS_PREPARED_FRAMES` is defined. Do not prepare while another thread pushes to the same analyzer.

A frame larger than `TILETRAIL_SAMPLE_LIMIT` (256) pixels on its longer side is point-sampled to that size before measurement. The header documents the rule. A caller that has already sampled a frame that way can submit the small frame, with the source’s rotation and pixel aspect, and gets the same result without handing over the full picture.
