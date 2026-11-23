# Metal workspace and host uploads

The Metal backend reuses a workspace for each analyzer. On Apple Silicon, mapped primary buffers let WGSL read the populated host buffer and write the mapped result buffer directly. The staged path remains available when the adapter lacks that capability. The host copies only rows used by nearest-center sampling. WGSL selects columns and unpacks RGB. The packed RGB pixel payload falls from 6,220,800 to 829,440 bytes per frame at 1920×1080 and from 1,555,200 to 414,720 bytes at 960×540. Metal polls briefly for short submissions before entering wgpu's blocking wait.

## Paired measurement

Both release binaries used the same dependencies, detector and tracker. Each run read packed 960×540 RGB24 frames, forced Metal, processed two loops at 10 source frames per second and finalized the tracks. The table shows medians from three alternating before/after pairs on an Apple M4 Max running macOS 27.0.1. Elapsed time includes initialization, raw frame reads and analysis. Decoding takes place before timing. Peak process RSS comes from `/usr/bin/time -l`.

| Fixture | Frames per run | Original elapsed (s) | Updated elapsed (s) | Original peak RSS (MiB) | Updated peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Synthetic interview | 88 | 3.503 | 3.234 | 37.94 | 35.09 |
| Synthetic gallery | 104 | 5.506 | 5.976 | 37.77 | 34.83 |
| Synthetic stage and share | 90 | 3.863 | 3.832 | 38.67 | 34.56 |
| Synthetic speech control | 48 | 2.091 | 2.113 | 38.25 | 35.45 |
| Jitsi gallery video | 60 | 4.458 | 4.765 | 38.44 | 32.58 |

Peak RSS fell by 2.8–5.9 MiB. Wall time improved for the interview and stage/share and increased for the gallery, speech control and Jitsi. Concurrent builds pushed host load averages above 100 during timing. These results leave the general throughput effect unresolved. Automatic selection still prefers Metal on Apple Silicon with the optional GPU feature. [Timing samples and input hashes](results/metal-workspace-2026-11-23.json)

Original Metal, updated Metal and CPU produced identical analysis JSON after removing backend diagnostics for all five timed fixtures. The Metal tests also exercise byte-exact sampled pixels, workspace resizing, staged transfers and recovery after device loss or a readback panic.

## Reproduce

Build `gallery_benchmark` with `cargo build --locked --release --features gpu --example gallery_benchmark` from each GPU revision against the same detector and tracker. Copy each binary to a temporary path before building the other variant.

```sh
python3 benchmarks/make_gallery.py /tmp/tiletrail-gallery-fixtures
python3 benchmarks/compare_builds.py /tmp/before_gallery /tmp/after_gallery /tmp/tiletrail-gallery-fixtures /tmp/tiletrail-metal-pair.json --modes gpu --loops 2 --repetitions 3
ffmpeg -v error -i validation/real/media/jitsi-gallery.mp4 -vf fps=10,scale=960:540 -pix_fmt rgb24 -f rawvideo /tmp/jitsi-gallery.rgb
/usr/bin/time -l /tmp/before_gallery gpu /tmp/jitsi-gallery.rgb 960 540 10 2
/usr/bin/time -l /tmp/after_gallery gpu /tmp/jitsi-gallery.rgb 960 540 10 2
```

Reverse the real-video run order for the second pair and restore the original order for the third. Append a result JSON path to either binary invocation to retain analysis output. Remove the `diagnostics` field before comparing backends.
