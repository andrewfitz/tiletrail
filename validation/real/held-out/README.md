# Held-out real recordings

Three additional excerpts from three different recording sessions cover a Google Meet screen share with a webcam thumbnail, a Microsoft Teams window shared back into the call, and a Zoom screen share with a strip of camera-off portraits and a live webcam.

From the repository root, use the real recording scorer with this directory.

```sh
python3 validation/real/score.py target/release/libtiletrail.dylib --dataset validation/real/held-out
```

Verify the packaged labels and clips with `shasum -a 256 -c SHA256SUMS` from this directory.

The manual labels cover two frames per excerpt. The Teams example labels the two full webcam boxes and the separate avatar tile in the shared Teams window, as well as the duplicate webcam at the right of the outer composition. Round icons in the Teams participants strip have no distinct tile borders and are excluded. The Zoom geometry includes the black side margins of its camera-off tiles and excludes the presenter embedded in the shared video. The labels evaluate geometry and camera-state classification at selected frames. They do not establish identity continuity, change timing, platform-wide accuracy, or coverage adequate for redaction.

## Results

Each run used the release CPU library. The matching threshold was IoU ≥ 0.50, with one-to-one matches. The figures for IoU and kind agreement cover matched tiles only. A dash indicates that the clip had zero matches.

| Run | Session | TP | FP | FN | Mean matched IoU | Kind agreement |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| [Baseline `ddec86e`](pre-tuning-metrics.json) | meet-holdout | 0 | 4 | 2 | — | — |
| [Baseline `ddec86e`](pre-tuning-metrics.json) | teams-holdout | 1 | 4 | 7 | 0.976 | 1 / 1 |
| [Baseline `ddec86e`](pre-tuning-metrics.json) | zoom-holdout | 2 | 14 | 8 | 0.991 | 2 / 2 |
| [Baseline `ddec86e`](pre-tuning-metrics.json) | All sessions | 3 | 22 | 17 | 0.986 | 3 / 3 |
| [First frozen revision `d9798c1`](first-sealed-metrics.json) | meet-holdout | 0 | 2 | 2 | — | — |
| [First frozen revision `d9798c1`](first-sealed-metrics.json) | teams-holdout | 0 | 6 | 8 | — | — |
| [First frozen revision `d9798c1`](first-sealed-metrics.json) | zoom-holdout | 2 | 6 | 8 | 0.991 | 2 / 2 |
| [First frozen revision `d9798c1`](first-sealed-metrics.json) | All sessions | 2 | 14 | 18 | 0.991 | 2 / 2 |
| [Final frozen revision `f1cdfb6`](final-metrics.json) | meet-holdout | 2 | 0 | 0 | 0.983 | 2 / 2 |
| [Final frozen revision `f1cdfb6`](final-metrics.json) | teams-holdout | 0 | 6 | 8 | — | — |
| [Final frozen revision `f1cdfb6`](final-metrics.json) | zoom-holdout | 2 | 6 | 8 | 0.991 | 2 / 2 |
| [Final frozen revision `f1cdfb6`](final-metrics.json) | All sessions | 4 | 12 | 16 | 0.987 | 4 / 4 |

## Sources and reuse

- **Google Meet**, `meet-holdout.mp4` is from *Abstract Wikipedia Volunteer Corner 2024-01.webm* by **Quiddity (WMF)** on January 8, 2024. The event announcement links to the Google Meet room. The excerpt starts at 718 seconds and was resized to 1180 × 720 to preserve the source’s display aspect on square pixels. [Recording and CC BY-SA 4.0 license](https://commons.wikimedia.org/w/index.php?title=File:Abstract_Wikipedia_Volunteer_Corner_2024-01.webm&oldid=1004710056) · [Event announcement](https://www.wikifunctions.org/wiki/Wikifunctions:Project_chat/Archive/2024/01#Wikifunctions_&_Abstract_Wikipedia_Newsletter_137_is_out:_Introducing_our_first_new_type:_Lists)
- **Microsoft Teams**, `teams-holdout.mp4` is from *USAWC – Keynote Speaker, Dr. Michael Hitt, Mays Business School | Texas A&M University – Strategic Leadership: A Multilevel and Multilayered Phenomenon*, posted by **U.S. Army War College Public Affairs** and recorded January 11, 2023. It starts at 37 seconds. DVIDS identifies the work as public domain and links to its reuse notice. [Recording and status](https://www.dvidshub.net/video/1020603/usawc-keynote-speaker-dr-michael-hitt-mays-business-school-texas-am-university-strategic-leadership-multilevel-and-multilayered-phenomenon) · [DVIDS reuse notice](https://www.dvidshub.net/about/copyright)
- **Zoom**, `zoom-holdout.mp4` is from *Wikimedia CEE Online Meeting 2020, Day 3 (Part 4), Russian* by **Frhdkazan** on October 4, 2020. It starts at 61 seconds. The conference guide confirms Zoom. [Recording and CC BY-SA 4.0 license](https://commons.wikimedia.org/wiki/File:CEEM2020-10-04-Part4-RU.webm) · [Conference guide](https://meta.wikimedia.org/wiki/Wikimedia_CEE_Online_Meeting_2020/Guide)

The appearance of U.S. Department of War (DoW) visual information does not imply or constitute DoW endorsement.

The Google Meet and Zoom excerpts carry the [Creative Commons Attribution-ShareAlike 4.0 International license](https://creativecommons.org/licenses/by-sa/4.0/). All three excerpts are trimmed, muted, and re-encoded to H.264. See [sources.json](sources.json) for source downloads and reproduction arguments.
