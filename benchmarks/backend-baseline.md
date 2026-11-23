# Host-frame backend baseline

Apple M4 Max, macOS 27.0.1 (26A434), release Rust build with the `gpu` feature. The median of five timed scans follows one warm-up scan. Each scan submits 12 frames at evenly spaced source timestamps and finalizes an analysis result. The timer includes frame validation, measurement, GPU upload and readback, detection, tracking, and result creation. A separate timer measures warm initialization. Fixture generation, media decoding, RGB-to-luma conversion and upscaling happen before the timed scan. Per-transfer time and peak GPU memory are unmeasured.

```sh
python3 benchmarks/make_gallery.py /tmp/tiletrail-gallery-fixtures
cargo run --locked --release --features gpu --example benchmark_backend -- /tmp/tiletrail-gallery-fixtures/gallery.rgb
cargo run --locked --release --features gpu --example benchmark_backend -- /tmp/tiletrail-gallery-fixtures/interview.rgb
```

| Fixture | Input dimensions | Format | CPU ms/frame | Metal ms/frame |
| --- | --- | --- | ---: | ---: |
| Gallery | 960×540 | RGB24 | 2.439 | 6.973 |
| Gallery | 960×540 | Gray8 | 0.811 | 8.187 |
| Gallery | 1920×1080 | RGB24 | 0.884 | 8.011 |
| Gallery | 1920×1080 | Gray8 | 0.831 | 5.956 |
| Interview | 960×540 | RGB24 | 0.267 | 5.131 |
| Interview | 960×540 | Gray8 | 0.250 | 5.321 |
| Interview | 1920×1080 | RGB24 | 0.319 | 6.814 |
| Interview | 1920×1080 | Gray8 | 0.260 | 5.554 |

Metal initialization took 0.75–1.14 ms for these warm runs. The Metal path performed the frame downsampling and RGB unpacking. Both paths used the same Rust detector and tracker. Both produced the same track and event counts for a given input and format. Timings varied under background build load. For example, the gallery RGB24 CPU measurement at 960×540 ranged from 1.021 to 2.709 ms/frame over three runs. CPU was faster for every measured case across those runs.
