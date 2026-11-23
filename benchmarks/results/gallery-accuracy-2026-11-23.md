# Synthetic gallery accuracy

All 949 annotated boxes matched at IoU ≥ 0.5 on the evaluated release, with zero false boxes. CPU and Metal produced identical observations, participant and appearance tracks, events, and coverage across the six clips. The raw inputs are deterministic RGB24 frames at 960×540 and 10 fps. Four development clips match the initial input SHA-256 hashes. The two held-out layouts were kept out of detector tuning.

The final release at `f1cdfb6292f7` reproduced the analytical payloads of the
accuracy freeze at `d9798c1` for every clip and backend. The comparison excludes
backend diagnostics. [Final metrics, input hashes, and parity](gallery-release-2026-11-23.json) · [Accuracy freeze metrics](gallery-freeze-2026-11-23.json)

| Clip | Boxes matched, initial → evaluated | False boxes, initial → evaluated | Mean matched IoU, evaluated | Annotated crop area covered, initial → evaluated |
| --- | ---: | ---: | ---: | ---: |
| Speech control | 48/48 → 48/48 | 0 → 0 | 0.993 | 99.63% → 99.63% |
| Interview | 76/76 → 76/76 | 0 → 0 | 0.993 | 99.63% → 99.63% |
| Dense gallery | 302/374 → 374/374 | 36 → 0 | 0.983 | 80.02% → 99.02% |
| Stage and share | 156/156 → 156/156 | 42 → 0 | 0.980 | 93.49% → 99.50% |
| Held-out reflow | 120/120 | 0 | 0.986 | 99.35% |
| Held-out share strip | 175/175 | 0 | 0.981 | 99.15% |

The highest matched crop edge error was 42.75 source pixels in the stage-and-share clip, down from 183.75 pixels initially. Crop area coverage is the area intersecting matched geometry divided by annotated area. Privacy export still needs pixel-level verification on rendered frames.

| Clip | Confirmed exact events / expected | False confirmed events | Provisional events | Returns resolved on first frame / expected |
| --- | ---: | ---: | ---: | ---: |
| Speech control | 0/0 | 0 | 0 | 0/0 |
| Interview | 7/7 | 0 | 0 | 2/2 |
| Dense gallery | 23/29 | 0 | 8 | 3/4 |
| Stage and share | 7/7 | 0 | 0 | 0/0 |
| Held-out reflow | 11/13 | 0 | 3 | 1/2 |
| Held-out share strip | 2/2 | 0 | 0 | 0/0 |

All 50 matched confirmed events landed on the exact annotated frame. The other eight expected events had no confirmed match. The gallery initially had 25 exact confirmed matches, six false confirmed events and five provisional events. The stage-and-share clip retained all seven exact matches and eliminated eight false confirmed events. The speech control had no events in either release. Screen-share regions had no participant IDs.

The interview recovered both the one-frame and sustained returns, compared with zero of two initially. No confirmed wrong links or fragmented links were observed on geometrically matched boxes in the evaluated build. The 20 indistinguishable gallery camera-off cards that previously had confirmed fragmented links all became provisional. The held-out reflow produced six additional provisional links. The stage plus thumbnail shared one participant ID with distinct appearance IDs in 25/25 development and 12/12 held-out person-frames. The fixtures exercise controlled layouts and support separate accuracy measurements on human recordings.

FFmpeg decoded a standard H.264 `yuv420p` copy of the interview for a separate check.
Both final release backends produced the same analytical results, with 76/76 boxes,
7/7 exact events and 2/2 identity returns. A lossless H.264 RGB copy decoded to
the original fixture's exact RGB24 bytes. These compressed checks used the
standalone release runner after FFmpeg decoding.
