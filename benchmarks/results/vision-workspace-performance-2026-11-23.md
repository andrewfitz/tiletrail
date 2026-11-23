# CPU sampling, corner filmstrips and Metal workspace reuse

Apple M4 Max, macOS 27.0.1, Rust 1.99.0. The baseline was the archived accuracy freeze at `d9798c1`. Both builds used locked dependencies and release optimization. CPU runs isolate the new sampler and filmstrip bounds. Fixed-shape Metal runs also include the workspace change. Other concurrent accuracy and history changes were excluded from the binaries.

## Packed RGB analysis

Each trial read packed RGB24 frames at 960×540 and built the full analysis. Times include analyzer creation, reads, frame analysis and finish. FFmpeg decoded the public three-second Jitsi screen-share clip before timing. Peak RSS covers the executable and its runtime. Process CPU combines user and system time and includes the small external timing wrapper. Measurements alternate build order across five pairs per case. CPU used 15 repeats of the input per run, Metal used five.

| Backend and input | Frames/run | Wall seconds, before → after | CPU seconds, before → after | Peak RSS MiB, before → after |
|---|---:|---:|---:|---:|
| cpu/interview | 660 | 1.463 → 1.403 | 0.230 → 0.219 | 8.92 → 8.98 |
| cpu/speech control | 360 | 0.978 → 0.464 | 0.141 → 0.124 | 8.72 → 8.73 |
| cpu/gallery | 780 | 4.279 → 4.242 | 0.465 → 0.459 | 9.64 → 9.62 |
| cpu/stage and share | 675 | 2.029 → 1.525 | 0.269 → 0.262 | 9.22 → 9.23 |
| cpu/jitsi | 90 | 0.469 → 0.339 | 0.090 → 0.071 | 9.41 → 9.38 |
| gpu/gallery | 260 | 1.341 → 1.337 | 0.308 → 0.291 | 32.42 → 32.75 |
| gpu/jitsi | 30 | 0.261 → 0.232 | 0.080 → 0.084 | 31.97 → 33.58 |

The host was running concurrent compiles and scans. The load average exceeded 200 early in the run. Treat small differences and wall-time magnitudes as uncertain. In three shorter CPU pairs, interview wall time and CPU time rose, and speech-control CPU time rose. The longer CPU runs showed median process time reductions of 1.4–20.5%. RSS differences on CPU stayed within 0.07 MiB. RSS does not measure GPU residency or driver allocations.

## Hot paths

In a two-second `sample` profile of the frozen CPU executable on the development Jitsi clip, pixel sampling occupied 445 of 1,411 samples and corner-filmstrip search occupied 431. The sampler now computes integer center-column offsets once per frame. The filmstrip search stops scanning a candidate when even perfect support on every remaining sample cannot beat the required score. This preserves prefix scores and tie-breaking for every winning candidate.

An in-process probe compared the same functions, alternating order, with one warm-up pair and six timed pairs. The sampler used a preloaded 960×540 patterned frame and returned its 256×144 measurements to `black_box`. The filmstrip probe ran the six premeasured public Jitsi frames. It invoked search directly on all six.

| Operation | Frames per trial | Median CPU ms before | Median CPU ms after |
|---|---:|---:|---:|
| Rgb24 sampling | 320 | 22.55 | 14.85 |
| Gray8 sampling | 320 | 16.48 | 7.66 |
| Rgba8 sampling | 320 | 15.92 | 11.26 |
| Bgra8 sampling | 320 | 21.48 | 12.02 |
| Filmstrip search | 192 | 64.20 | 30.78 |

## Changing input shapes

The previous Metal workspace kept the exact sizes from the last allocation. A wide frame can need a larger upload buffer while a square frame needs a larger pixel-output buffer. Alternating those shapes rebuilt the workspace at every push. The workspace now keeps the maximum required capacity independently for each buffer. A real Metal test reproduced the lost capacity in the previous build and passed with the change.

The public Python ABI benchmark alternated zero-filled 960×540 and 256×256 RGB24 frames for 200 pushes. Both variants used the optimized CPU vision code and forced Metal. Five alternating pairs gave median push times of 0.958 → 0.647 seconds wall and 0.167 → 0.112 seconds process CPU. Peak RSS including Python and finish was 36.53 → 36.34 MiB. All output digests matched. The capacity rule reduces workspace creation from 200 times to twice for this sequence. On mapped Metal, retained buffer capacities total 676,884 bytes after both shapes, up from the previous single-workspace maximum of 562,196 bytes. This trades 112 KiB of bounded capacity for reuse.

## Output checks

Complete CPU JSON was byte-identical to the freeze for the four development workloads, the public Jitsi clip, and four generated stress streams. The stress inputs covered odd and even corner positions, portrait, landscape and one-pixel axes. Required Metal matched the full CPU JSON after removing the backend diagnostics field. Hardware tests also exercised all four formats, row padding, direct and staged buffers, device destruction and automatic CPU fallback. The CPU regression independently checked every sample against the scalar integer formula on non-square, thin and padded frames.

## Reproduction

Build each variant with `cargo build --locked --release --features gpu --lib --example gallery_benchmark`. Save the corresponding example and dynamic library under distinct paths. Generate the four synthetic development inputs with `benchmarks/make_gallery.py`.

```sh
ffmpeg -v error -i validation/real/media/jitsi-gallery-share.mp4 -vf fps=2,scale=960:540 -pix_fmt rgb24 -f rawvideo "$fixtures/jitsi.rgb"
python3 benchmarks/compare_builds.py "$before" "$after" "$fixtures" cpu.json --modes cpu --loops 15 --repetitions 5 --cases interview speech_control gallery stage_and_share jitsi
python3 benchmarks/compare_builds.py "$before" "$after" "$fixtures" gpu.json --modes gpu --loops 5 --repetitions 5 --cases gallery jitsi
python3 benchmarks/measure_resize.py "$library" --frames 200
```

The raw comparison includes input and binary digests, individual timings and parity hashes. [Benchmark results](vision-workspace-performance-2026-11-23.json)
