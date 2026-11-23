"""Score participant continuity and timed change events in real recordings."""

import argparse
import collections
import ctypes as c
import hashlib
import json
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
sys.path.insert(0, str(HERE.parent / "real"))
from score import match
from analyze import Buffer, Config, Error, Frame, Timestamp, check, load_library


def verify_inputs():
    for line in (HERE / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        expected, filename = line.split(maxsplit=1)
        if hashlib.sha256((HERE / filename).read_bytes()).hexdigest() != expected:
            raise ValueError(f"fixture checksum mismatch: {filename}")


def analyze_clip(library, fixture, case, fps):
    first, last = fixture["scan_ticks"]
    dropped = set(case["drop_ticks"])
    width, height = fixture["size"]
    pixels = bytearray(width * height * 3)
    data = (c.c_uint8 * len(pixels)).from_buffer(pixels)
    config = Config(1, int(bool(dropped)))  # CPU, every frame or sparse
    analyzer, error, output = c.c_void_p(), Error(), Buffer()
    check(library.tiletrail_analyzer_create(c.byref(config), Timestamp(first, 1, fps),
                                           c.byref(analyzer), c.byref(error)), error)
    try:
        command = ["ffmpeg", "-nostdin", "-v", "error", "-i",
                   str(HERE / "media" / fixture["clip"]), "-an", "-frames:v", str(last + 1),
                   "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"]
        with subprocess.Popen(command, stdout=subprocess.PIPE) as decoder:
            for tick in range(last + 1):
                if decoder.stdout.readinto(pixels) != len(pixels):
                    raise ValueError(f"{fixture['clip']} ended at tick {tick}")
                if tick < first or tick in dropped:
                    continue
                frame = Frame(data, len(pixels), width * 3, width, height, 1, 0, 1, 1,
                              Timestamp(tick, 1, fps))
                check(library.tiletrail_analyzer_push(analyzer, c.byref(frame), c.byref(error)),
                      error)
            if decoder.wait():
                raise RuntimeError(f"FFmpeg failed on {fixture['clip']}")
        check(library.tiletrail_analyzer_result_json(analyzer, Timestamp(last + 1, 1, fps),
                                                   c.byref(output), c.byref(error)), error)
        return json.loads(c.string_at(output.data, output.len))
    finally:
        library.tiletrail_buffer_free(c.byref(output))
        library.tiletrail_analyzer_free(analyzer)


def match_anchors(result, fixture, case, threshold):
    width, height = fixture["size"]
    observations = {frame["timestamp"]["pts"]: frame for frame in result["observations"]}
    matches = {}
    counts = collections.Counter()
    for tick_text, layout_name in fixture["anchors"].items():
        tick = int(tick_text)
        if tick in case["drop_ticks"]:
            continue
        layout = fixture["layouts"][layout_name]
        labels = [{"id": person, "box": box} for person, box in layout.items()]
        regions = [region for region in observations[tick]["regions"]
                   if region["kind"] != "screen_share"]
        boxes = []
        for region in regions:
            rect = region["rect"]
            box = (rect["x"] * width, rect["y"] * height,
                   rect["width"] * width, rect["height"] * height)
            boxes.append((box, region["kind"]))
        paired = match(labels, boxes, threshold)
        matches[tick] = {labels[i]["id"]: regions[j] for j, (i, _) in paired.items()}
        camera_states = fixture["camera_states"]
        expected_states = {**camera_states["default"], **camera_states.get(tick_text, {})}
        counts.update(
            labeled=len(labels),
            matched=len(paired),
            camera_state_matches=sum(region["kind"] == expected_states[person]
                                     for person, region in matches[tick].items()),
        )
    return matches, dict(counts)


def continuity(matches, fixture):
    counts = collections.Counter()
    anchors = sorted(matches)
    for before, after in zip(anchors, anchors[1:]):
        first = fixture["layouts"][fixture["anchors"][str(before)]]
        second = fixture["layouts"][fixture["anchors"][str(after)]]
        for person in first.keys() & second.keys():
            a = matches[before].get(person)
            b = matches[after].get(person)
            if a is None or b is None:
                counts["missing_geometry"] += 1
            elif a["participant_id"] is None or b["participant_id"] is None:
                counts["missing_identity"] += 1
            else:
                same = a["participant_id"] == b["participant_id"]
                uncertain = a["identity_uncertain"] or b["identity_uncertain"]
                identity = "same_id" if same else "different_id"
                confidence = "provisional" if uncertain else "certain"
                counts[f"{identity}_{confidence}"] += 1
    return {"pairs": sum(counts.values()), **dict(sorted(counts.items()))}


def events(result, case, matches, tolerance):
    remaining = list(result["events"])
    outcomes = []
    for label in case["events"]:
        before, after = label["anchors"]
        a = matches[before].get(label["id"])
        b = matches[after].get(label["id"])
        outcome = {**label, "status": "missing_geometry"}
        if a is not None and b is not None:
            outcome["status"] = "missing_identity"
            if a["participant_id"] is not None and a["participant_id"] == b["participant_id"]:
                outcome["status"] = "missing_event"
                candidates = [
                    (abs(event["timestamp"]["pts"] - label["tick"]), position)
                    for position, event in enumerate(remaining)
                    if event["kind"] == label["kind"]
                    and event["participant_id"] == a["participant_id"]
                    and event["appearance_id"] == a["appearance_id"] == b["appearance_id"]
                    and abs(event["timestamp"]["pts"] - label["tick"]) <= tolerance
                ]
                if candidates:
                    _, index = min(candidates)
                    actual = remaining.pop(index)
                    outcome.update(status="matched",
                                   tick_error=actual["timestamp"]["pts"] - label["tick"],
                                   reported_uncertain=actual["uncertain"])
                    if label.get("uncertain") and not actual["uncertain"]:
                        outcome["status"] = "uncertainty_missing"
        outcomes.append(outcome)

    negative_counts = collections.Counter()
    for event in result["events"]:
        tick = event["timestamp"]["pts"]
        if any(start <= tick <= end for start, end in case["negative_windows"]):
            confidence = "uncertain" if event["uncertain"] else "certain"
            negative_counts[f"{event['kind']}_{confidence}"] += 1

    coverage = collections.Counter()
    for tick in case["drop_ticks"]:
        statuses = [interval["status"] for interval in result["coverage"]
                    if interval["start"]["pts"] <= tick < interval["end"]["pts"]]
        if len(statuses) != 1:
            raise ValueError(f"expected one coverage interval at dropped tick {tick}")
        coverage[statuses[0]] += 1
    return {
        "expected_events": len(outcomes),
        "event_outcomes": dict(sorted(collections.Counter(o["status"] for o in outcomes).items())),
        "negative_window_events": sum(negative_counts.values()),
        "negative_window_breakdown": dict(sorted(negative_counts.items())),
        "dropped_tick_coverage": dict(sorted(coverage.items())),
        "details": outcomes,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    args = parser.parse_args()
    verify_inputs()
    labels = json.loads((HERE / "labels.json").read_text(encoding="utf-8"))
    library = load_library(args.library)
    reports = {}
    for fixture in labels["fixtures"]:
        for case in fixture["cases"]:
            result = analyze_clip(library, fixture, case, labels["frames_per_second"])
            matches, anchor_counts = match_anchors(result, fixture, case, labels["iou_threshold"])
            reports[case["name"]] = {
                "anchors": anchor_counts,
                "continuity": continuity(matches, fixture),
                **events(result, case, matches, labels["event_tolerance_ticks"]),
            }
    print(json.dumps(reports, indent=2))


if __name__ == "__main__":
    main()
