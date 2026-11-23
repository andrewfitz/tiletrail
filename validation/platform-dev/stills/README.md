# Gallery screenshots for spatial scoring

Two Google Meet images show a 24-tile gallery at different moments of the same event. Two Microsoft Teams images show a 2-by-2 grid and an asymmetric three-person gallery from different events. The samples were encoded as one-second still-image MP4s for compatibility with the existing scorer. Each file contributes just one labeled frame. Use this set for spatial scoring.

```sh
python3 validation/real/score.py target/release/libtiletrail.dylib --dataset validation/platform-dev/stills
```

Gallery labels include camera-off tiles, each tile's name tag, and colored active-speaker borders. Meeting controls and the attendee icons in the bottom bar of the Teams three-person gallery are interface elements. The partly obscured Navy gallery boxes approximate the visible outer bounds.

## Sources and reuse

- Meet screenshots 01 and 05 were taken by Sachin Patel 78 at Wiki Club SATI's World Book Day meeting on April 23, 2026. The encoded copies are under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/). [Screenshot 01](https://commons.wikimedia.org/wiki/File:Screenshot_of_an_online_meeting_session_of_Wiki_Club_SATI_conducted_via_Google_Meet_on_World_Book_Day,_showing_multiple_participants_actively_engaged_in_the_discussion_01.png) · [Screenshot 05](https://commons.wikimedia.org/wiki/File:Screenshot_of_an_online_meeting_session_of_Wiki_Club_SATI_conducted_via_Google_Meet_on_World_Book_Day,_showing_multiple_participants_actively_engaged_in_the_discussion_05.png)
- The Teams 2-by-2 gallery is credited to Navy Supply Corps School staff. The photograph depicts the Senior Supply Officer Department Head Course and appears on printed page 28 of *The Navy Supply Corps Newsletter*, Fall 2020. [Original publication](https://media-cdn.dvidshub.net/pubs/pdf_56251.pdf#page=32)
- The Teams three-person gallery was published by the 311th Signal Command (Theater) Public Affairs Team for “Team Yeti Celebrates Barbara Flynn.” It appears on page 42 of *Premier Signal*, Summer 2020. [Original publication](https://media-cdn.dvidshub.net/pubs/pdf_55704.pdf#page=42)

The Teams images were extracted from the original U.S. government publications and padded on the bottom or right by one pixel to meet the encoder's requirements. The appearance of U.S. Department of War (DoW) visual information does not imply or constitute DoW endorsement. [DVIDS public use notice](https://www.dvidshub.net/about/copyright)
