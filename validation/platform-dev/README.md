# Google Meet and Microsoft Teams development recordings

This small development set contains real calls used to tune participant-tile detection. It uses the same label and scorer format as [real recording validation](../real/README.md). Run from the repository root with a built TileTrail library.

```sh
python3 validation/real/score.py target/release/libtiletrail.dylib --dataset validation/platform-dev
```

The scorer samples at 4 fps. Manual participant boxes are in excerpt pixels as `[left, top, width, height]` at 1 and 3 seconds into each clip. Each box marks the outer bounds of one visible tile. Shared content, embedded portraits, and meeting controls are excluded. In the Teams filmstrip, the four named entries with rectangular backgrounds are camera-off tiles. The additional unframed attendee icons and the overflow counter are interface elements. The remote room feed is one video tile even when it contains several people.

A separate [gallery screenshot set](stills/README.md) adds spatial samples of tiled layouts for both platforms.

## Development results

| Samples | Labeled regions | Earlier matches / extras | Updated matches / extras | Updated mean matched IoU |
| --- | ---: | ---: | ---: | ---: |
| 16 frames from moving clips | 30 | 8 / 16 | 30 / 0 | 0.970 |
| Four gallery screenshots | 55 | 38 / 25 | 55 / 0 | 0.964 |

The earlier CPU detector at `d9798c1` classified the camera state correctly for 8 of 8 matched moving-clip regions and 22 of 38 matched screenshot regions. The updated detector at `3e60bfc` classified all 85 matched regions correctly. These are development measurements on the layouts used to refine the detector. [Per-clip results and CPU timings](../../benchmarks/results/platform-development-2026-11-23.json)

## Sources and reuse

- The Google Meet speaker and camera-on sidebar excerpts come from the March 4, 2024 Abstract Wikipedia Volunteer's Corner by Sannita (WMF), under CC BY-SA 4.0. [Recording and license](https://commons.wikimedia.org/wiki/File:Abstract_Wikipedia_Volunteer_Corner_2024-03.webm) · [Platform](https://commons.wikimedia.org/wiki/Category:Abstract_Wikipedia_Volunteer%27s_Corner)
- The Google Meet camera-off sidebar is from the September 2, 2024 Abstract Wikipedia Volunteer's Corner by Sannita (WMF), under CC BY-SA 4.0. [Recording and license](https://commons.wikimedia.org/wiki/File:Abstract_Wikipedia_Volunteer_Corner_2024-09.webm)
- The Teams speaker, sidebar, and slides with embedded portraits are from the November 5, 2020 AFCEA Maritime IT Summit, produced by the Department of the Navy Chief Information Officer. DVIDS identifies the work as public domain and the event as a Microsoft CVR Teams conference. [Recording and rights](https://www.dvidshub.net/video/772313/afcea-maritime-summit)
- The Teams stage and presenter overlay excerpts come from the May 25, 2021 DON IT Conference East Coast Cyber Workforce Track, published by the Department of the Navy Chief Information Officer. DVIDS identifies the work as public domain. [Recording and rights](https://www.dvidshub.net/video/801433/don-conference-east-coast-may-25-cyber-workforce-track) · [Platform](https://www.doncio.navy.mil/%28ah2jtj21pduikr45nkdiyl55%29/ContentView.aspx?id=14597)

The clips were trimmed, muted, sampled at 12 fps, and re-encoded as H.264 at the published rendition's size. The Meet excerpts retain the source's [CC BY-SA 4.0 license](https://creativecommons.org/licenses/by-sa/4.0/). See [sources.json](sources.json) for the exact source renditions, clip offsets and command.

The appearance of U.S. Department of War (DoW) visual information does not imply or constitute DoW endorsement. [DVIDS public use notice](https://www.dvidshub.net/about/copyright)
