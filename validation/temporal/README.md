# Temporal validation on real recordings

These cases exercise participant identity and timestamped events on two public development recordings. They use different excerpts of the same source recordings as [real geometry development validation](../real/README.md). The labels and media were hashed before the first detector run.

Build the CPU library and run from the repository root with Python 3 and FFmpeg.

```sh
cargo build --release --no-default-features
python3 validation/temporal/score.py target/release/libtiletrail.dylib
```

Use `libtiletrail.so` on Linux or `tiletrail.dll` on Windows. The scorer validates [SHA256SUMS](SHA256SUMS), streams the packaged 8 fps clips through the public C ABI, and reports JSON. No downloads are needed.

## Observed transitions

| Case | Clip-relative ticks at 8 Hz | Human observations |
| --- | --- | --- |
| Jitsi dense | 6–36, change at 9 (1.125 s) | An admitted screen share appears as a new grid cell. Denny remains stationary. The other nine participants move, and the three participants in the new bottom row resize. The name tag for James F. stays visible as his tile presents a camera-off avatar on ticks 9–10, then returns to video at tick 11 (1.375 s). |
| Jitsi sparse | 6–36, omit 9–19 | The input brackets the reflow between ticks 8 and 20. The expected move and resize events report uncertainty at the first submitted post-change frame (2.5 s). The transient avatar occurs entirely inside the omission and is excluded from expected events. |
| Zoom dense | 0–63, share changes at ticks 16 and 55 | The slide editor goes black, then a full-screen presentation appears. Camera thumbnails remain in the same right-hand strip with the same ordering; the active speaker border can change. |

Annotations in [labels.json](labels.json) describe visible UI. Names and positions link Jitsi tiles across the reflow. For Zoom, appearance and stable ordering link the three thumbnail occupants. No inference about network presence or whether a physical camera was toggled is involved. The Jitsi bottom-row bounds approximate the visible outer bounds behind the toolbar. Nested tiles inside the shared screen are excluded.

## How to read the results

Participant boxes at labeled anchors match one-to-one at IoU ≥ 0.50. The continuity counts compare each person across consecutive submitted anchors, distinguishing missing geometry, missing IDs, provisional continuity, confident continuity, and ID changes. Camera-state totals are evaluated on matched boxes.

A positive event must match both anchor boxes, preserve participant and appearance IDs across those anchors, match the event kind, and fall within one tick (125 ms) of the label. A sparse match must report event uncertainty. The scorer distinguishes event misses from anchor failures and lists timing errors for matches. These positive scores cover the annotated Jitsi reflow and avatar transition. Extra events during the reflow are outside the precision count.

Every reported participant event in the stable windows is counted and split by uncertainty. The Jitsi windows begin at tick 12 for dense input and tick 21 for sparse input. The Zoom window begins at tick 1 to exclude scan initialization. Sparse coverage is separately counted over the eleven omitted ticks. The geometry correspondence requirement means that a genuine event can fail because of a detector miss, lost identity, or reset appearance before the event itself is considered.

This small sample covers one real grid reflow, one brief visible camera-state transition, and screen-share content changes. It does not estimate temporal accuracy across platforms, genuine joins or departures, long occlusions, or cross-layout re-identification.

## Results

CPU builds of `ddec86e` and `d9798c1` were scored on macOS arm64 with FFmpeg 9.0.2. The raw reports include one record per expected event. [Initial report](results/ddec86e.json) · [Revised detector report](results/d9798c1.json)

| Case and measure | Initial | Revised |
| --- | ---: | ---: |
| Jitsi dense, matched boxes | 58/90 | 82/90 |
| Jitsi dense, same-ID anchor pairs (certain) | 47/80 (47) | 63/80 (28) |
| Jitsi dense, matched timed events | 4/14 | 1/14 |
| Jitsi dense, events in stable window | 24 | 12 |
| Jitsi sparse, matched boxes | 30/50 | 50/50 |
| Jitsi sparse, same-ID anchor pairs (certain) | 21/40 (21) | 33/40 (19) |
| Jitsi sparse, matched timed and uncertainty-marked events | 5/12 | 2/12 |
| Jitsi sparse, omitted ticks with inferred coverage | 11/11 | 11/11 |
| Jitsi sparse, events in stable window | 24 | 0 |
| Zoom dense, matched boxes | 7/24 | 24/24 |
| Zoom dense, same-ID anchor pairs (certain) | 1/21 (1) | 19/21 (16) |
| Zoom dense, events in stable window | 24 | 22 |

Every matched event landed on its labeled tick. The revised build found all submitted Zoom and sparse Jitsi anchor boxes but still broke identities across the Jitsi reflow. It identified James F.'s avatar and video states on matched anchors and missed both corresponding timed events. Neither build timed the three Jitsi resizes successfully. Zoom video geometry stayed stable at the labeled anchors, yet the revised build emitted 22 events during the stable interval, including 13 marked certain. The sparse test verifies that the two matched moves are marked uncertain after a controlled omission.

## Attribution and reproduction

- **Jitsi**. `jitsi-share-launch.mp4` excerpts *Abstract Wikipedia Volunteer Corner 2023-09.webm* by **DVrandecic (WMF)**, starting 114 seconds into the original. [Recording and license](https://commons.wikimedia.org/w/index.php?title=File:Abstract_Wikipedia_Volunteer_Corner_2023-09.webm&oldid=1004710052)
- **Zoom**. `zoom-slide.mp4` excerpts *RU-07112021-WMCEEOM-Track1-LocalRecording-5.webmhd.webm* by **Frhdkazan**, starting 38 seconds into the original. [Recording and license](https://commons.wikimedia.org/w/index.php?title=File:RU-07112021-WMCEEOM-Track1-LocalRecording-5.webmhd.webm&oldid=1026242580)

The source recordings and excerpts are licensed under [Creative Commons Attribution-ShareAlike 4.0 International](https://creativecommons.org/licenses/by-sa/4.0/). The excerpts were trimmed, muted, resampled to 8 fps and re-encoded. Source URLs and SHA-256 hashes are recorded in [labels.json](labels.json). Run the following for each source using its listed start offset. Encoded output bytes can vary with FFmpeg and x264 versions.

```sh
ffmpeg -ss SOURCE_START_SECONDS -i SOURCE_FILE -t 8 -an -vf fps=8 \
  -c:v libx264 -preset fast -crf 17 -pix_fmt yuv420p -movflags +faststart CLIP
```
