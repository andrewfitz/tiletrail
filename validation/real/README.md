# Real recording validation

The development set scores participant tile geometry and camera-state classification on five three-second excerpts from two public videoconference recordings.

With Python 3 and FFmpeg on PATH, build the CPU library and run from the repository root.

```sh
cargo build --release --no-default-features
python3 validation/real/score.py target/release/libtiletrail.dylib
```

Use `target/release/libtiletrail.so` on Linux or `target/release/tiletrail.dll` on Windows. The scorer decodes at four frames per second and uses the public C ABI through the repository's Python example. It needs no network access. By default it reports at intersection over union (IoU) of 0.50. Use `--iou` to change the threshold.

## Labels and scope

The manual labels mark the outside bounds of visible participant tiles at one and two seconds into each excerpt. The coordinates in [labels.json](labels.json) are in native-resolution pixels as `[left, top, width, height]`. They include avatars, name tags, and visible portions of partly obscured tiles. Gallery toolbars, chat, screen shares, and other application UI are excluded from participant labels. Labels on the bottom Jitsi row approximate visible outer bounds where the controls obscure part of the row.

Each sampled frame uses maximum-cardinality one-to-one matching above the IoU threshold. Any detector region classified as video, camera off, or unknown is a participant candidate. Camera-state accuracy is computed on matched participant boxes. IoU and camera-state averages describe matched boxes only. The full-screen share is a negative control for participant detections.

The two samples per excerpt characterize the selected layouts and share false positives. They do not estimate platform-wide accuracy or measure event timing, identity tracking, or redaction. The local label IDs are annotation handles. The Zoom excerpts show participant strips during a screen share, including a slide transition. The gallery excerpts are from Jitsi. The separate evaluation sets include Google Meet, Microsoft Teams, and an additional Zoom recording.

## Recorded results

The final frozen CPU detector at `f1cdfb6292f7` was scored at IoU ≥ 0.50. The development samples were available for tuning. The first held-out set was scored at three frozen revisions. A second pair of recordings was first scored after the final accuracy freeze.

| Set | Frames | Matched / labeled | False positives | Mean matched IoU | Kind agreement |
| --- | ---: | ---: | ---: | ---: | ---: |
| Zoom and Jitsi development recordings | 10 | 54 / 54 | 4 | 0.971 | 54 / 54 |
| Meet and Teams development recordings | 16 | 30 / 30 | 0 | 0.970 | 30 / 30 |
| First held-out recordings | 6 | 4 / 20 | 12 | 0.987 | 4 / 4 |
| Final verification recordings | 4 | 2 / 4 | 2 | 0.994 | 2 / 2 |

The Teams sessions in both reserved sets yielded no matched boxes. The first held-out Zoom excerpt recovered its one live webcam in both frames with no matches on the camera-off boxes. The reported mean IoU and camera-state agreement apply only to detections that matched a human label.

[Zoom and Jitsi development per-clip metrics](final-development-metrics.json) · [Meet and Teams development per-clip metrics](final-platform-development-metrics.json) · [First held-out results and attribution](held-out/README.md) · [Final verification results and attribution](final-only/README.md)

## Attribution

- **Jitsi**, `jitsi-gallery.mp4`, `jitsi-gallery-share.mp4`, and `jitsi-share-only.mp4` come from *Abstract Wikipedia Volunteer Corner 2023-09.webm* by **DVrandecic (WMF)**, September 18, 2023. From the original recording, the clips start at 8, 116, and 124 seconds respectively. The event announcement confirms Jitsi for this session. [Recording and license](https://commons.wikimedia.org/w/index.php?title=File:Abstract_Wikipedia_Volunteer_Corner_2023-09.webm&oldid=1004710052) · [Event announcement](https://www.wikifunctions.org/wiki/Wikifunctions:Project_chat/Archive/2023/09#Next_Wikifunctions_&_Abstract_Wikipedia_Volunteer's_Corner_on_Monday_18)
- **Zoom**, `zoom-three-thumbnails.mp4` and `zoom-four-thumbnails.mp4` come from *RU-07112021-WMCEEOM-Track1-LocalRecording-5.webmhd.webm* by **Frhdkazan**, from Wikimedia CEE Online Meeting 2021. From the original recording, the clips start at 40 and 88 seconds. [Recording and license](https://commons.wikimedia.org/w/index.php?title=File:RU-07112021-WMCEEOM-Track1-LocalRecording-5.webmhd.webm&oldid=1026242580) · [Conference report](https://diff.wikimedia.org/2022/01/06/wikimedia-cee-online-meeting-2021-a-gateway-to-a-better-future/)

The recordings and video excerpts in `media/` are licensed under [Creative Commons Attribution-ShareAlike 4.0 International](https://creativecommons.org/licenses/by-sa/4.0/). Changes to the source recordings are trimming, audio removal, and re-encoding to H.264. The excerpt manifest includes the original file hashes and FFmpeg command for reproduction.
