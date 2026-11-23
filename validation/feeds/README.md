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

On Linux, use `target/release/libtiletrail.so`. The scorer uses the CPU through the public C ABI. The sample media stays in the local build directory.
