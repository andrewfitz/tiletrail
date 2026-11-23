#!/usr/bin/env python3
"""Score one TileTrail analysis against synthetic frame annotations."""

import argparse
from collections import Counter, defaultdict
from fractions import Fraction
import json
from pathlib import Path


IOU_THRESHOLD = 0.5
MOTION_THRESHOLD = 0.025


def overlap(a, b):
    x, y, w, h = a
    u, v, width, height = b
    area = max(0, min(x + w, u + width) - max(x, u)) * max(0, min(y + h, v + height) - max(y, v))
    union = w * h + width * height - area
    return area / union if union > 0 else 0


def associate(actual, expected):
    """Maximize the number of accepted pairs, then their summed IoU."""
    states = {0: (0, 0.0, [])}
    for i, rect in enumerate(actual):
        choices = [(j, overlap(rect, target)) for j, target in enumerate(expected)]
        next_states = states.copy()
        for mask, (count, total, pairs) in states.items():
            for j, iou in choices:
                bit = 1 << j
                if iou < IOU_THRESHOLD or mask & bit:
                    continue
                new_mask = mask | bit
                candidate = (count + 1, total + iou, pairs + [(i, j, iou)])
                if new_mask not in next_states or candidate[:2] > next_states[new_mask][:2]:
                    next_states[new_mask] = candidate
        states = next_states
    return max(states.values(), key=lambda value: value[:2])[2]


def source_frame(stamp, fps):
    clock = stamp["time_base"]
    return Fraction(stamp["pts"] * clock["numerator"] * fps, clock["denominator"])


def normalized(rect, width, height):
    return tuple(value / scale for value, scale in zip(rect, (width, height, width, height)))


def expected_events(frames):
    events = []
    previous = {}
    for index, current in frames.items():
        current = {(tile["person"], tile["role"]): tile for tile in current if tile["person"] is not None}
        old_people = {person for person, role in previous}
        people = {person for person, role in current}
        if index:
            events.extend((index, person, "enter") for person in sorted(people - old_people))
            events.extend((index, person, "leave") for person in sorted(old_people - people))
        for key in current.keys() & previous.keys():
            before, after = previous[key], current[key]
            x, y, w, h = before["normalized"]
            u, v, width, height = after["normalized"]
            kinds = []
            if max(abs((x + w / 2) - (u + width / 2)), abs((y + h / 2) - (v + height / 2))) > MOTION_THRESHOLD:
                kinds.append("move")
            if max(abs(w - width), abs(h - height)) > MOTION_THRESHOLD:
                kinds.append("resize")
            if before["kind"] != after["kind"]:
                kinds.append("camera_state")
            events.extend((index, key[0], kind) for kind in kinds)
        previous = current
    return sorted(events)


def ratio(a, b):
    return round(a / b, 4) if b else None


def event_errors(confirmed, expected):
    """Pair events in time order within each participant and event kind."""
    actual_by_key, expected_by_key = defaultdict(list), defaultdict(list)
    for tick, person, kind in confirmed:
        actual_by_key[person, kind].append(tick)
    for tick, person, kind in expected:
        expected_by_key[person, kind].append(tick)

    errors = []
    for key, desired in expected_by_key.items():
        desired.sort()
        row = [()] * (len(desired) + 1)
        for tick in sorted(actual_by_key[key]):
            next_row = [()]
            for column, target in enumerate(desired):
                options = [row[column + 1], next_row[column]]
                distance = abs(tick - target)
                if distance <= 1:
                    options.append((*row[column], float(distance)))
                next_row.append(max(options, key=lambda distances: (len(distances), -sum(distances))))
            row = next_row
        errors.extend(row[-1])
    return errors


def score(truth, result):
    width, height, fps = truth["width"], truth["height"], truth["fps"]
    expected = {
        frame["frame"]: [dict(tile, normalized=normalized(tile["rect"], width, height)) for tile in frame["tiles"]]
        for frame in truth["frames"]
    }
    phases = {frame["frame"]: frame.get("phase", "unspecified") for frame in truth["frames"]}
    actual = {source_frame(frame["timestamp"], fps): frame["regions"] for frame in result["observations"]}
    if len(actual) != len(result["observations"]):
        raise ValueError("detector emitted duplicate observation timestamps")
    counts = Counter()
    moments = {}
    ious = []
    first_track, owner = {}, {}
    previously_present, seen = set(), set()
    by_kind = {kind: Counter() for kind in ("video", "camera_off", "screen_share")}
    by_phase = defaultdict(Counter)
    for index in sorted(expected.keys() | actual.keys()):
        targets, predictions = expected.get(index, []), actual.get(index, [])
        pairs = associate(
            [tuple(region["rect"][key] for key in ("x", "y", "width", "height")) for region in predictions],
            [tile["normalized"] for tile in targets],
        )
        visible = {tile["person"] for tile in targets if tile["person"] is not None}
        returns = (visible - previously_present) & seen
        counts["returns"] += len(returns)
        resolved_returns = set()
        seen.update(visible)
        previously_present = visible
        counts["boxes"] += len(targets)
        counts["predictions"] += len(predictions)
        counts["matches"] += len(pairs)
        phase = by_phase[phases.get(index, "extra_timestamps")]
        phase["boxes"] += len(targets)
        phase["missed"] += len(targets) - len(pairs)
        phase["false"] += len(predictions) - len(pairs)
        for tile in targets:
            by_kind[tile["kind"]]["boxes"] += 1
        for i, j, iou in pairs:
            region, tile = predictions[i], targets[j]
            ious.append(iou)
            by_kind[tile["kind"]]["matched"] += 1
            by_kind[tile["kind"]]["correct_kind"] += region["kind"] == tile["kind"]
            counts["roles_correct"] += region["role"] == tile["role"]
            identity, person = region["participant_id"], tile["person"]
            if person is None:
                counts["screen_share_with_identity"] += identity is not None
                continue
            counts["identity_candidates"] += 1
            uncertain = region.get("identity_uncertain")
            if uncertain is None:
                uncertain = region["identity_confidence"] is None and any(
                    option["participant_id"] != identity for option in region["possible_matches"]
                )
            if uncertain:
                counts["uncertain_links"] += 1
                continue
            if identity is None:
                counts["unassigned_links"] += 1
                continue
            if identity in owner and owner[identity] != person:
                counts["wrong_links"] += 1
                continue
            owner.setdefault(identity, person)
            had_reference = person in first_track
            first_track.setdefault(person, identity)
            if first_track[person] != identity:
                counts["fragmented_links"] += 1
                continue
            if had_reference and person in returns:
                resolved_returns.add(person)
        counts["unmatched_returns"] += len(returns - resolved_returns)
        if len(targets) != len(pairs) or len(predictions) != len(pairs):
            moments[str(index)] = {"missed": len(targets) - len(pairs), "false": len(predictions) - len(pairs)}
    want = expected_events(expected)
    confirmed = []
    uncertain_events = 0
    for event in result["events"]:
        tick = source_frame(event["timestamp"], fps)
        if tick == 0 and event["kind"] == "enter":
            continue
        if event["uncertain"]:
            uncertain_events += 1
            continue
        confirmed.append((tick, owner.get(event["participant_id"]), event["kind"]))
    boundary_errors = event_errors(confirmed, want)

    return {
        "case": truth["case"],
        "source_frames": len(expected),
        "observed_frames": len(expected.keys() & actual.keys()),
        "extra_observation_timestamps": len(actual.keys() - expected.keys()),
        "box_precision_iou_0_5": ratio(counts["matches"], counts["predictions"]),
        "box_recall_iou_0_5": ratio(counts["matches"], counts["boxes"]),
        "mean_matched_box_iou": ratio(sum(ious), len(ious)),
        "missed_boxes": counts["boxes"] - counts["matches"],
        "false_boxes": counts["predictions"] - counts["matches"],
        "matched_boxes": counts["matches"],
        "boxes_by_kind": by_kind,
        "boxes_by_layout_phase": by_phase,
        "correct_role_on_matched_boxes": ratio(counts["roles_correct"], counts["matches"]),
        "identity_comparisons": counts["identity_candidates"],
        "confirmed_wrong_identity_links": counts["wrong_links"],
        "confirmed_fragmented_identity_links": counts["fragmented_links"],
        "uncertain_identity_links": counts["uncertain_links"],
        "unassigned_identity_links": counts["unassigned_links"],
        "screen_share_with_participant_id": counts["screen_share_with_identity"],
        "expected_returns": counts["returns"],
        "unmatched_returns": counts["unmatched_returns"],
        "expected_events": len(want),
        "false_confirmed_events": len(confirmed) - len(boundary_errors),
        "missed_events": len(want) - len(boundary_errors),
        "uncertain_events": uncertain_events,
        "event_precision_within_one_frame": ratio(len(boundary_errors), len(confirmed)),
        "event_recall_within_one_frame": ratio(len(boundary_errors), len(want)),
        "mean_event_boundary_error_frames": ratio(sum(boundary_errors), len(boundary_errors)),
        "max_event_boundary_error_frames": max(boundary_errors, default=None),
        "unpaired_boxes_by_frame": moments,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("truth", type=Path)
    parser.add_argument("analysis", type=Path)
    args = parser.parse_args()
    print(json.dumps(score(json.loads(args.truth.read_text()), json.loads(args.analysis.read_text())), indent=2))


if __name__ == "__main__":
    main()
