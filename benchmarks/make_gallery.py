#!/usr/bin/env python3
"""Generate deterministic RGB24 gallery clips with exact frame annotations."""

import argparse
import json
import math
from pathlib import Path

WIDTH, HEIGHT, FPS = 960, 540, 10
SCENARIOS = {"speech_control": 24, "interview": 44, "gallery": 52, "stage_and_share": 45}
HELD_OUT = {"heldout_reflow": 36, "heldout_share_strip": 35}
PHASES = {
    "speech_control": [(0, "two_up")],
    "interview": [(0, "two_up"), (8, "seat_swap"), (16, "camera_off"), (23, "one_visible"),
                  (30, "one_frame_return"), (31, "one_visible"), (36, "sustained_return")],
    "gallery": [(0, "full_grid"), (10, "partial_row"), (20, "centered_partial_row"),
                (30, "duplicate_camera_cards"), (40, "borderless_grid")],
    "stage_and_share": [(0, "speaker_stage"), (8, "stage_swap"), (18, "screen_share"),
                        (23, "controls_overlay"), (29, "screen_share"),
                        (30, "speaker_stage"), (37, "fullscreen_video")],
    "heldout_reflow": [(0, "offset_two_by_two"), (12, "wide_two_up"), (24, "offset_two_by_two")],
    "heldout_share_strip": [(0, "bottom_share_strip"), (11, "stage_with_duplicate"),
                            (23, "bottom_share_strip")],
}
PALETTES = [
    ((74, 102, 139), (189, 124, 91), (33, 76, 133), (42, 35, 33)),
    ((172, 150, 103), (128, 77, 55), (139, 47, 63), (33, 23, 26)),
    ((93, 146, 127), (229, 173, 131), (69, 48, 97), (110, 72, 48)),
    ((143, 112, 144), (162, 99, 77), (55, 101, 75), (34, 26, 24)),
    ((109, 133, 143), (238, 188, 159), (138, 88, 48), (137, 102, 73)),
    ((136, 108, 74), (158, 110, 80), (49, 83, 122), (21, 24, 23)),
    ((72, 123, 121), (211, 147, 112), (155, 94, 38), (87, 43, 23)),
    ((128, 101, 156), (150, 100, 84), (69, 81, 61), (39, 32, 32)),
    ((120, 128, 83), (230, 162, 123), (123, 56, 97), (107, 81, 62)),
]


def tile(person, rect, role="primary", kind="video", name=None):
    return dict(person=person, rect=rect, role=role, kind=kind,
                name=name if name else f"GUEST {person}")


def layout(case, tick):
    if case == "heldout_reflow":
        if 12 <= tick < 24:
            return [tile(4, (4, 111, 469, 264)), tile(1, (481, 111, 469, 264))]
        ids = (1, 2, 3, 4) if tick < 12 else (4, 3, 2, 1)
        return [
            tile(person, (112 + index % 2 * 358, 39 + index // 2 * 205, 349, 196),
                 kind="camera_off" if 24 <= tick < 30 and person == 3 else "video")
            for index, person in enumerate(ids)
        ]
    if case == "heldout_share_strip":
        main = (165, 0, 630, 354)
        thumbnail_ids = (1, 4, 8, 6) if tick < 23 else (1, 6, 8, 4)
        primary = tile(8, main) if 11 <= tick < 23 else tile(None, main, kind="screen_share")
        return [primary, *[
            tile(person, (38 + index * 224, 378, 203, 114), role="thumbnail")
            for index, person in enumerate(thumbnail_ids)
        ]]
    if case in ("speech_control", "interview"):
        ids = (1, 2) if case == "speech_control" or tick < 8 else (2, 1)
        left = tile(ids[0], (0, 135, 478, 269),
                    kind="camera_off" if case == "interview" and 16 <= tick < 23 else "video")
        right = tile(ids[1], (482, 135, 478, 269))
        if case == "interview" and (23 <= tick < 30 or 31 <= tick < 36):
            return [right]
        return [left, right]
    if case == "gallery":
        layouts = [
            (1, 2, 3, 4, 5, 6, 7, 8, 9),
            (6, 3, 8, 1, 9, 2, 4, 5),
            (3, 1, 6, 7, 5),
            (7, 1, 2, 3, 8, 9, 5),
        ]
        ids = layouts[min(tick // 10, 3)]
        cols, width, height = 3, 314, 176
        gap = 0 if tick >= 40 else 6
        rows = (len(ids) + cols - 1) // cols
        top = (HEIGHT - rows * height - (rows - 1) * gap) // 2
        tiles = []
        for row in range(rows):
            seats = ids[row * cols : (row + 1) * cols]
            left = (WIDTH - len(seats) * width - (len(seats) - 1) * gap) // 2
            for col, person in enumerate(seats):
                tiles.append(tile(
                    person, (left + col * (width + gap), top + row * (height + gap), width, height),
                    name="ALEX" if person in (2, 7) else None,
                    kind="camera_off" if 30 <= tick < 40 and person in (2, 7) else "video",
                ))
        return tiles
    if tick >= 37:
        return [tile(2, (0, 0, WIDTH, HEIGHT))]
    stage = (0, 66, 720, 405)
    if 18 <= tick < 30:
        main = tile(None, stage, kind="screen_share")
    else:
        main = tile(1 if tick < 8 else 2, stage)
    ids = (2, 3, 1) if tick < 18 else (3, 1, 2)
    return [main, *[
        tile(person, (728, 57 + row * 144, 226, 127), role="thumbnail")
        for row, person in enumerate(ids)
    ]]


def rect(frame, x, y, width, height, rgb):
    left, right = max(x, 0), min(x + width, WIDTH)
    if left >= right:
        return
    line = bytes(rgb) * (right - left)
    for row in range(max(0, y), min(HEIGHT, y + height)):
        start = (row * WIDTH + left) * 3
        frame[start : start + len(line)] = line


def ellipse(frame, x, y, width, height, color):
    for row in range(height):
        relative_y = (2 * row + 1 - height) / height
        radius = int(width * math.sqrt(1 - relative_y * relative_y) / 2)
        rect(frame, x + width // 2 - radius, y + row, max(1, radius * 2), 1, color)


def shade(color, amount):
    return tuple(max(0, min(255, channel + amount)) for channel in color)


def paint_tile(frame, region, tick):
    x, y, w, h = region["rect"]

    def box(px, py, pw, ph, color):
        left, top = px * w // 1000, py * h // 1000
        rect(frame, x + left, y + top, (px + pw) * w // 1000 - left,
             (py + ph) * h // 1000 - top, color)

    def oval(px, py, pw, ph, color):
        ellipse(frame, x + px * w // 1000, y + py * h // 1000,
                max(1, pw * w // 1000), max(1, ph * h // 1000), color)

    if region["kind"] == "screen_share":
        box(0, 0, 1000, 1000, (227, 233, 240))
        box(0, 0, 1000, 65, (37, 55, 82))
        box(20, 105, 235, 875, (189, 202, 223))
        for index in range(6):
            box(302, 250 + index * 74, 450 + index % 3 * 80, 13, (93, 112, 149))
        box(315, 126, 210, 42, (37, 55, 82))
        box(840, 720 + tick % 6 * 18, 22, 26, (221, 66, 59))
        return

    person = region["person"]
    background, skin, shirt, hair = PALETTES[person - 1]
    if region["kind"] == "camera_off":
        box(0, 0, 1000, 1000, (34, 36, 40))
        oval(427, 370, 146, 260, (77, 79, 84))
    else:
        for band in range(20):
            box(0, band * 50, 1000, 50, shade(background, 11 - band))
        window = 75 + person % 3 * 253
        box(window, 110, 250, 405, shade(background, 20))
        box(window + 12, 124, 230, 377, shade(background, 40))
        box(window + 115, 124, 12, 377, shade(background, 15))
        box(0, 720, 1000, 280, shade(background, -17))
        shift = (tick * 3 + person * 5) % 21 - 10
        oval(169 + shift, 589, 662, 410, shade(shirt, -13))
        oval(208 + shift, 610, 584, 382, shirt)
        box(423 + shift, 595, 152, 103, shade(skin, -22))
        oval(323 + shift, 168, 356, 490, hair)
        oval(354 + shift, 240, 292, 421, skin)
        oval(370 + shift, 255, 235, 377, shade(skin, 15))
        oval(344 + shift, 184, 308, 145, hair)
        for eye in (409, 553):
            eye_height = 7 if (tick + person * 2) % 13 == 0 else 26
            oval(eye + shift, 406, 43, eye_height, (46, 35, 34))
        oval(488 + shift, 422, 29, 88, shade(skin, -20))
        oval(441 + shift, 544, 118, 12 + (tick * (person + 1)) % 53, shade(skin, -66))
        if person % 2 == 0:
            box(367 + shift, 381, 267, 10, hair)
            box(384 + shift, 390, 6, 48, hair)
            box(607 + shift, 390, 6, 48, hair)
    # Stable label glyphs. Identical names are pixel-identical within a layout.
    box(12, 894, 252, 87, (20, 20, 24))
    for index, char in enumerate(region["name"]):
        for bit in range(7):
            if ord(char) & (1 << bit):
                box(28 + index * 24 + bit % 3 * 5, 914 + bit // 3 * 13, 5, 9, (236, 236, 239))


def paint_frame(case, tick, tiles):
    frame = bytearray(bytes((14, 15, 18)) * (WIDTH * HEIGHT))
    for region in tiles:
        paint_tile(frame, region, tick)
    if case == "stage_and_share" and 23 <= tick < 29:
        rect(frame, 213, 460, 523, 54, (47, 52, 60))
        for index in range(7):
            ellipse(frame, 245 + index * 69, 472, 31, 31,
                    (170, 51, 60) if index == 6 else (195, 201, 214))
    return frame


def write_case(output, case):
    truth = {"case": case, "width": WIDTH, "height": HEIGHT, "fps": FPS, "frames": []}
    with (output / f"{case}.rgb").open("wb") as raw:
        cases = SCENARIOS | HELD_OUT
        for tick in range(cases[case]):
            tiles = layout(case, tick)
            raw.write(paint_frame(case, tick, tiles))
            phase = next(name for start, name in reversed(PHASES[case]) if tick >= start)
            truth["frames"].append({"frame": tick, "phase": phase, "tiles": tiles})
    (output / f"{case}.truth.json").write_text(json.dumps(truth, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--case", choices=SCENARIOS | HELD_OUT, action="append")
    parser.add_argument("--held-out", action="store_true", help="generate validation clips only")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    for case in args.case or (HELD_OUT if args.held_out else SCENARIOS):
        write_case(args.output, case)


if __name__ == "__main__":
    main()
