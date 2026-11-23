# Gallery accuracy and performance

Generate four procedural webcam composites with exact frame-level annotations. The generator uses only the Python standard library and creates roughly 245 MiB of temporary packed RGB24 frames. Use a fresh directory for each benchmark result.

```sh
python3 benchmarks/make_gallery.py /tmp/tiletrail-gallery-fixtures
cargo build --release --example gallery_benchmark

target/release/examples/gallery_benchmark cpu \
  /tmp/tiletrail-gallery-fixtures/interview.rgb 960 540 10 1 \
  /tmp/tiletrail-gallery-fixtures/interview.analysis.json

python3 benchmarks/score_gallery.py \
  /tmp/tiletrail-gallery-fixtures/interview.truth.json \
  /tmp/tiletrail-gallery-fixtures/interview.analysis.json
```

| Clip | Source frames | Cases |
| --- | ---: | --- |
| `speech_control` | 24 | Stable two-person layout with animated speech and blinking |
| `interview` | 44 | Seat swap, camera-off card, leave/return, one-frame return |
| `gallery` | 52 | Nine participants, centered partial rows, reflows, borderless grid, indistinguishable camera-off cards and duplicate nameplates |
| `stage_and_share` | 45 | Stage swap, same person in two regions, screen share, control overlay, fullscreen video |

Generate the separately authored validation layouts with
`python3 benchmarks/make_gallery.py /tmp/tiletrail-gallery-fixtures --held-out`.
They add a 2×2 reflow with a different seat geometry and a horizontal
thumbnail strip below both a screen share and a video stage. Use the four
clips above for development and keep these two cases out of threshold tuning.

The composer annotates drawn regions independently of the detector. The scorer associates each frame's boxes one-to-one, maximizing the number of matches at IoU ≥ 0.5 and then the sum of their IoU. The scorer reports mean IoU over matched boxes, along with missed and false boxes and their timestamps.

A participant's first confirmed and geometrically matched ID becomes the reference for identity scoring. A subsequent different ID is a fragmented link if it has the same owner or no known owner. An ID previously attributed to another person is a wrong link. A return is resolved when the returning participant is matched to their original confirmed ID on the first visible frame. The scorer records uncertain decisions independently. Event scoring uses participant and kind, then matches within one source frame. Frame-zero enter events are omitted. Move and resize events use a minimum change of 2.5% of the corresponding normalized axis. The score is an offline diagnostic and has no pass threshold.

The portraits, rooms, labels, and presentation slide are drawn by the generator. Evaluate permissioned human recordings separately to measure accuracy on actual meetings. Keep the benchmark fixture and scorer fixed when comparing detectors. If this corpus is used to tune detector thresholds, use a different evaluation corpus to measure generalization.

## Frozen release accuracy

Run all six cases through one GPU-enabled release binary. This checks exact
CPU/Metal result parity except for backend diagnostics, including comparisons
of observations, participant and appearance tracks, events, and coverage.

```sh
python3 benchmarks/make_gallery.py /tmp/tiletrail-gallery-fixtures
python3 benchmarks/make_gallery.py /tmp/tiletrail-gallery-fixtures --held-out
cargo build --locked --release --features gpu --example gallery_benchmark
python3 benchmarks/run_gallery_accuracy.py target/release/examples/gallery_benchmark \
  /tmp/tiletrail-gallery-fixtures /tmp/tiletrail-gallery-raw-results \
  /tmp/tiletrail-gallery-accuracy.json --revision "$(git rev-parse HEAD)"
```

Crop edge error is measured in source pixels for matched rectangles.
Annotated crop coverage divides the sum of intersections for matched rectangles
by the total annotated area, including missed rectangles. This is a geometry
measurement. It needs a separate rendered-output test for privacy export.

## Release timing

The [initial gallery snapshot](results/gallery-initial-2026-11-23.json) records
the first pre-fix CPU results, fixture and executable hashes, and exploratory
Metal timing. The initial CPU and Metal builds used different tracker revisions.

On macOS, run `/usr/bin/time -l` and read maximum resident set size in bytes. On Linux, run `/usr/bin/time -v` and read maximum resident set size in KiB. Omit the result path for timing. This removes JSON serialization from peak RSS. Each run streams input frames through one reusable buffer. Elapsed time includes backend initialization, reading packed RGB24 frames, processing them, and committing the result. Read/push, startup, and finish times are also reported individually. The current benchmark finalizes a borrowed snapshot. At revision `d9798c1`, finalization cloned the complete history into an independent result. Both versions serialize the full result after the elapsed-time measurement when an output path is supplied. The [C snapshot comparison](results/snapshot-performance-2026-11-23.md) measures export including JSON serialization and peak process RSS.

```sh
/usr/bin/time -l target/release/examples/gallery_benchmark cpu \
  /tmp/tiletrail-gallery-fixtures/interview.rgb 960 540 10 100
/usr/bin/time -l target/release/examples/gallery_benchmark cpu \
  /tmp/tiletrail-gallery-fixtures/gallery.rgb 960 540 10 10

cargo build --release --features gpu --example gallery_benchmark
/usr/bin/time -l target/release/examples/gallery_benchmark gpu \
  /tmp/tiletrail-gallery-fixtures/interview.rgb 960 540 10 100
/usr/bin/time -l target/release/examples/gallery_benchmark gpu \
  /tmp/tiletrail-gallery-fixtures/gallery.rgb 960 540 10 10
```

Loops are repeated through one analyzer with a continuous presentation clock, so they also exercise track retention and cross-loop layout changes. Measure OS-level peak RSS separately for each run. RSS includes the benchmark process's host allocations. Use a device profiler for GPU allocation and transfer measurements.

To exercise a compressed-input pipeline, wrap the same RGB frame sequence in a lossless H.264 video using FFmpeg. The annotations remain valid, and `fps` stays 10.

```sh
ffmpeg -f rawvideo -pixel_format rgb24 -video_size 960x540 -framerate 10 \
  -i /tmp/tiletrail-gallery-fixtures/interview.rgb -c:v libx264rgb -crf 0 \
  /tmp/tiletrail-gallery-fixtures/interview.mp4
```
