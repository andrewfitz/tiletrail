# Initial and final release binaries

On the same M4 Max, five alternating pairs ran six loops of each 960×540 packed RGB workload against the initial release build and final detector. The timer includes analyzer initialization, frame reads, analysis and snapshot construction. Input decoding and output encoding are outside this timer. CPU and Metal use the same host frames. Real footage is scored separately. [Independent recording results](../../validation/real/README.md)

| Workload | Elapsed seconds, initial → final | Process CPU seconds, initial → final | Peak process RSS MiB, initial → final |
| --- | ---: | ---: | ---: |
| CPU interview | 0.094 → 0.060 | 0.098 → 0.065 | 8.53 → 8.53 |
| CPU gallery | 0.285 → 0.121 | 0.288 → 0.126 | 9.50 → 9.14 |
| CPU stage and share | 0.190 → 0.084 | 0.179 → 0.084 | 8.97 → 8.53 |
| CPU speech control | 0.055 → 0.032 | 0.059 → 0.036 | 8.41 → 8.41 |
| GPU interview | 1.243 → 0.119 | 0.267 → 0.138 | 34.55 → 31.95 |
| GPU gallery | 1.868 → 0.633 | 0.589 → 0.297 | 35.09 → 34.05 |
| GPU stage and share | 1.745 → 0.550 | 0.516 → 0.205 | 35.00 → 31.95 |
| GPU speech control | 0.858 → 0.393 | 0.293 → 0.130 | 34.83 → 33.44 |

Every measured median elapsed time decreased. CPU analysis was faster than Metal on all four host-RGB workloads, including the final gallery. MPCut still prefers Metal in automatic mode on Apple Silicon. CPU-only mode is available for these workloads. Earlier isolated Metal changes produced mixed results. [Metal workspace comparison](../metal-workspace.md)

A stress test deliberately permuted similar avatar cards on every frame. The tracker retained provisional candidates for review. At 1,000 frames and 64 cards, the follow-on accuracy changes raised peak RSS from 75.6 to 96.9 MiB and elapsed time from 1.47 to 1.93 seconds. This workload produced 4,060 track records. Full observation history grows with scan length and ambiguity.

Initial source `ddec86e`. Final source `f1cdfb6292f7`. [Raw runs, binary and input checksums](end-to-end-final-2026-11-23.json)
