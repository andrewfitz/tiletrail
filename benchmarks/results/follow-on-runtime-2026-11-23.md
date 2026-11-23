# Runtime and parity after the platform and identity changes

On an Apple M4 Max, both the CPU and required Metal backend produced identical analysis after dropping backend diagnostics. The check covered eight moving Meet and Teams development clips, four gallery screenshots encoded as clips, two Zoom and Jitsi temporal clips, and a generated 64-card stress stream. Public clips were sampled at 4 fps. The maximum detected region counts were 24 on the dense Meet screenshot and 64 on the stress stream.

## Release benchmark

The baseline was `b1136ac` and the candidate was `f1cdfb6`. Both runners used the GPU feature and locked dependencies. Each row shows medians from three alternating pairs on macOS 27.0.1 with Rust 1.99.0. We timed raw RGB reads, setup, all frame pushes and finish in one process per run. CPU time covers user and system time for the process and its timing wrapper. Peak RSS includes the result history. Video decoding and physical GPU memory are outside the measurement.

| Workload | Backend | Frames | Wall ms before → after | Process CPU ms before → after | Peak RSS MiB before → after |
|---|---|---:|---:|---:|---:|
| meet-grid | cpu | 100 | 103 → 105 | 108 → 110 | 13.33 → 13.55 |
| meet-grid | gpu | 40 | 73 → 75 | 82 → 83 | 36.31 → 36.31 |
| meet-sidebar | cpu | 400 | 119 → 118 | 124 → 124 | 8.22 → 8.31 |
| meet-sidebar | gpu | 160 | 98 → 94 | 104 → 105 | 31.75 → 32.34 |
| teams-overlay | cpu | 400 | 216 → 181 | 220 → 186 | 13.02 → 13.23 |
| teams-overlay | gpu | 160 | 140 → 155 | 154 → 143 | 38.02 → 36.27 |
| active-avatars-64 | cpu | 100 | 90 → 103 | 95 → 108 | 14.22 → 16.70 |
| active-avatars-64 | gpu | 40 | 85 → 87 | 73 → 78 | 34.50 → 35.47 |

The synthetic stress contains four 256×256 RGB24 frames. Each frame has an 8×8 grid of 32-pixel cells. A one-pixel black gutter surrounds each gray tile, whose central 14×14 icon cycles through the same 64 color codes across slots. The analysis measures ambiguity and assignment costs when similar cards move continually.

On a separate 1,000-frame CPU run of this stress, the candidate took 1.93 seconds and peaked at 96.9 MiB, up from 1.47 seconds and 75.6 MiB. It recorded 4,060 tracks, including provisional identities, and 67,932 events. The baseline held 64 fixed-slot tracks and generated zero events. This workload has no identity labels. Repeated ambiguity can fragment history and raise the memory needed to retain it. The detector caps its pixel grid at 256×256 and its output at 64 tiles per frame. The assignment considers up to 64 active and 128 dormant candidates. The full retained history grows with scan length.

## Correctness checks

Three isolated regressions reproduced and then passed. A shared virtual background had incorrectly confirmed a replacement at 0.72 confidence. Two identical patterned thumbnails had vetoed a three-slot layout. Gallery cleanup had discarded a recognized document beside nine thumbnails. The revised tracker requires independent pixel evidence, and grid cleanup now preserves a recognized document outside the candidate grid. A separate probe remeasured 700 display-coordinate crops across four rotations and five sampled dimensions, including single-pixel axes, with exact rectangle, descriptor and kind round trips.

The all-feature suite passed 44 unit tests, seven marker integration tests and two documentation tests, including real Metal device-loss and staged-buffer checks. Strict all-target Clippy passed. [Raw samples and parity counts](follow-on-runtime-2026-11-23.json)
