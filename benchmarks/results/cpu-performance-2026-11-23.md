# CPU performance pass, November 23, 2026

The release detector ran 1.44–1.78× faster across four development clips on an Apple M4 Max. All 12 paired trials favored the optimized build. All four one-loop analysis files and accuracy scores matched byte for byte. The run used the four default clips from the gallery generator.

| Clip | Frames per process | Median seconds before → after | Median FPS before → after | Peak RSS MiB before → after |
| --- | ---: | ---: | ---: | ---: |
| Interview | 528 | 2.445 → 1.378 | 216.0 → 383.1 | 4.66 → 4.64 |
| Gallery | 624 | 7.156 → 4.985 | 87.2 → 125.2 | 5.48 → 5.41 |
| Stage and share | 540 | 3.815 → 2.143 | 141.5 → 251.9 | 4.72 → 4.88 |
| Speech control | 288 | 0.985 → 0.560 | 292.4 → 514.6 | 4.44 → 4.45 |

| Clip | Box precision at IoU ≥ 0.5 | Box recall at IoU ≥ 0.5 | Mean matched-box IoU |
| --- | ---: | ---: | ---: |
| Interview | 1.0000 | 1.0000 | 0.9927 |
| Gallery | 0.8935 | 0.8075 | 0.9846 |
| Stage and share | 0.7879 | 1.0000 | 0.9582 |
| Speech control | 1.0000 | 1.0000 | 0.9927 |

The baseline gallery process ran under macOS `sample` for three seconds at a 1 ms interval. The call tree captured 1,842 samples and attributed 1,471 leaf samples to `vision::detect`, which includes inlined split scanning. Direct tracker leaf samples were comparatively sparse. The detector now stops checking a matte line after its count of out-of-range pixels exceeds the existing 4% allowance. The split scorer reads the neighboring lines only where the main gap meets the existing salience floor. Both changes preserve the score and all thresholds for every candidate. The detector-only binaries precede the query experiment.

## Repeat the detector measurement

Generate the standard fixtures with `python3 benchmarks/make_gallery.py /tmp/tiletrail-gallery-fixtures`. Build `gallery_benchmark` with `cargo build --release --example gallery_benchmark` and save one binary with the two scanner changes removed and one with them applied. Preserve the same tracker, vision audit, and query code in both builds. The immediate baseline included the earlier correctness changes atop `ddec86e`. The machine used macOS 27.0.1 arm64 and rustc 1.99.0.

```sh
python3 benchmarks/compare_builds.py /tmp/before /tmp/after \
  /tmp/tiletrail-gallery-fixtures /tmp/detector.json \
  --modes cpu --repetitions 3 --loops 12
```

For output parity, run each binary with a single loop and a destination JSON path, then compare the two JSON files directly. Feed both into `benchmarks/score_gallery.py` with the same matching truth file and compare the score files. The raw timing file includes per-run timings, peak process RSS, and binary and RGB fixture SHA-256 hashes. The parity file records the analysis and score SHA-256 hashes for both builds. [Run measurements](cpu-vision-2026-11-23.json) · [Parity checksums](cpu-parity-2026-11-23.json)

## Repeat the visibility-query measurement

Generate a JSON snapshot by running the gallery benchmark on `gallery.rgb` for 15 loops at 960 × 540 and 10 fps. The resulting 780-observation snapshot is common to both binaries. Deserialize it before timing. Compile the following loop with `rustc --edition=2024 -O`, linked to each build's release `tiletrail` and `serde_json` libraries. In each run, pass a denominator of either 10 or 1000 for `rate`. Use `/usr/bin/time -l` to collect process CPU time, retired instructions, and peak RSS. Run three trials per binary and alternate which binary runs first.

```rust
use std::{hint::black_box, time::Instant};
use tiletrail::{AnalysisResult, Timestamp};

fn main() {
    let path = std::env::args_os().nth(1).unwrap();
    let file = std::fs::File::open(path).unwrap();
    let result: AnalysisResult =
        serde_json::from_reader(std::io::BufReader::new(file)).unwrap();
    let count = result.observations.len();
    let rate: u32 = std::env::args().nth(2).unwrap().parse().unwrap();
    let start = Instant::now();
    let mut checksum = 0;
    for iteration in 0..10_000_000 {
        let index = iteration * 1031 % count;
        let pts = index as i64 * i64::from(rate / 10);
        let instant = black_box(Timestamp::new(pts, 1, rate));
        if let Some(frame) = black_box(&result).visible_at(instant) {
            checksum += frame.regions.len();
        }
    }
    println!("{} {checksum}", start.elapsed().as_secs_f64());
}
```

The second query prototype compared ticks directly for identical valid time bases, canceled a shared positive numerator for different denominators, and removed an already checked scan-range guard from the coverage lookup. It retired fewer instructions, but elapsed and CPU time rose in both cases, so the implementation was reverted. Each run returned a checksum of 65,000,000. Concurrent builds made wall time particularly noisy.

| Query clock | Median wall seconds before → prototype | Median CPU user seconds before → prototype | Median retired instructions before → prototype | Peak RSS MiB before → prototype |
| --- | ---: | ---: | ---: | ---: |
| 1/10, same clock | 10.687 → 10.733 | 0.61 → 0.63 | 5.862B → 4.356B | 3.19 → 3.14 |
| 1/1000, mixed clocks | 14.809 → 15.391 | 0.65 → 0.68 | 5.882B → 5.182B | 3.14 → 3.20 |

The first prototype only accelerated identical time bases and removed the same guard. Its median wall time improved from 4.091 to 3.378 seconds for the same clock and worsened from 7.381 to 13.243 seconds for mixed clocks, with substantial run-to-run variation. [Query measurements](cpu-query-2026-11-23.json)
