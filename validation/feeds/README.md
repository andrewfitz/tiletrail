# Detecting feeds in a composited recording

The scorer creates six short H.264 recordings from four moving camera sequences. Every frame has known display-space rectangles. A frame scores a pass when the detected count matches and every expected rectangle has intersection over union of at least 0.90 with a detection. The swap case separately checks both identities on every frame. Input footage comes from the [Xiph test media collection](https://media.xiph.org/video/derf/).

| Recording | Meeting profile | Automatic feeds | Automatic mean IoU |
| --- | ---: | ---: | ---: |
| Four views, 8 px gutters | 11/12 | 12/12 | 0.995 |
| Four views, touching edges | 5/12 | 12/12 | 0.988 |
| One large and two small views | 9/12 | 12/12 | 0.991 |
| Single camera control | 7/12 | 12/12 | 1.000 |
| Two cameras changing sides | 11/12 | 12/12 | 0.986 |
| Bordered picture-in-picture | 0/12 | 12/12 | 0.984 |

The automatic profile followed both cameras across the swap. The meeting profile lost continuity on that clip. These are development fixtures built from the same four source sequences. Borderless adjacent images with similar content can give too little evidence to locate their seam. A cut can also make stream identity ambiguous. A known grid supplies deterministic cell geometry in these cases.

Run from the crate root with Python 3 and FFmpeg (including libx264). The script downloads the first two MiB of each source into the local target directory and uses original decoded frames to compose the test inputs.

```sh
cargo build --release --features gpu
python3 validation/feeds/score.py target/release/libtiletrail.dylib
python3 validation/feeds/score.py target/release/libtiletrail.dylib --meeting
```

The same six recordings were also decoded through the MPCut native frame tap. All 72 native frames had the expected feed count. The compressed inset retained a stable ID on all 12 frames. To exercise the chroma interpolation that exposed the inset regression, add `--nv12` to the scorer.

On Linux, use `target/release/libtiletrail.so`. The scorer uses the CPU through the public C ABI by default. Pass `--gpu` to require hardware acceleration. On Apple Silicon, the CPU and Metal produced identical observations, tracks, appearances, events and coverage on all six clips in automatic mode and again in 2×2 fixed-grid mode. The sample media stays in the local build directory.

The macOS and Linux CPU test suites passed, including the geometry and identity regressions. All eight new feed and inset integration tests also passed in Windows x86-64 GNU executables under Wine. The maximum grid size is a geometry limit. Every-frame scan results retain a region observation for each visible feed in each sampled frame, so long recordings and large grids can require substantial memory.

## Dense gallery regression

Run `cargo test --test gallery`. The fixture generates a full 5 × 5 gallery
with labeled source-local patterns. It moves the first roster, replaces
15 sources while keeping ten on screen, then brings the original roster
back. The same fixture runs with every-frame and sparse analysis. A second
fixture fills all 64 cells of an 8 × 8 gallery.

Pass thresholds are zero missed boxes, zero false or duplicate boxes, and
zero source-to-feed identity changes or feed IDs shared by different
sources. A match must be within 3 coded pixels of the labeled origin and
4 pixels of the labeled width and height. These are regression thresholds
for the deterministic fixtures.
