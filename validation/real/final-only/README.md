# Final verification recordings

Two publicly released recordings from sessions distinct from the development and first held-out corpora. Each excerpt shows a webcam thumbnail beside a screen share. The labels mark the outside bounds of the webcam thumbnail at one and two seconds into the clip. Gallery-style Meet and Teams layouts are outside these two clips’ coverage.

From the repository root, run the scorer with this directory.

```sh
python3 validation/real/score.py target/release/libtiletrail.dylib --dataset validation/real/final-only
```

The media and manual annotations were fixed and checksummed before the final detector run. Verify them with `shasum -a 256 -c SHA256SUMS` from this directory.

## Results

Each run used the release CPU library. The matching threshold was IoU ≥ 0.50, with one-to-one matches. The figures for IoU and kind agreement cover matched tiles only. A dash indicates that the clip had zero matches.

| Run | Session | TP | FP | FN | Mean matched IoU | Kind agreement |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| [Final frozen revision `f1cdfb6`](final-metrics.json) | meet-final-only | 2 | 0 | 0 | 0.994 | 2 / 2 |
| [Final frozen revision `f1cdfb6`](final-metrics.json) | teams-final-only | 0 | 2 | 2 | — | — |
| [Final frozen revision `f1cdfb6`](final-metrics.json) | All sessions | 2 | 2 | 2 | 0.994 | 2 / 2 |

## Sources and reuse

- **Google Meet**, `meet-final-only.mp4` is from *MoveComms office hour (report findings) (2021-06-10 at 05 06 GMT-7).webm* by **Elena Lappen**, June 10, 2021. The source metadata identifies a Google Meet recording. The excerpt begins at 178 seconds and carries the [Creative Commons Attribution-ShareAlike 3.0 Unported license](https://creativecommons.org/licenses/by-sa/3.0/). The slide includes *Merging in the mist* by **F Mira**, with the original attribution visible. [Recording](https://commons.wikimedia.org/w/index.php?title=File:MoveComms_office_hour_%28report_findings%29_%282021-06-10_at_05_06_GMT-7%29.webm&oldid=847097805) · [Photograph and license](https://commons.wikimedia.org/wiki/File:Ponte_Vasco_da_Gama_25.jpg)
- **Microsoft Teams**, `teams-final-only.mp4` is from *Visitor Access Management Plan – Virtual Public Meeting, August 22, 2024*, credited to the **National Park Service**. The meeting notice identifies Microsoft Teams. The excerpt begins at 29 seconds and shows the park’s opening slide and presenter. NPS identifies its own material as generally public domain unless otherwise indicated. [Recording and credit](https://www.nps.gov/media/video/view.htm?id=D34A0094-A9B7-435B-9246-8F95B488CDE3) · [Meeting notice](https://parkplanning.nps.gov/MeetingNotices.cfm?projectID=113113) · [NPS copyright policy](https://www.nps.gov/aboutus/disclaimer.htm)

Both excerpts were trimmed, muted, and re-encoded to H.264. The source URLs and reproduction arguments are in [sources.json](sources.json).
