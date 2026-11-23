"""Score TileTrail on manually labeled frames from real videoconferences."""

import argparse
import collections
import json
import pathlib
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.dont_write_bytecode = True
sys.path.insert(0, str(HERE.parent.parent / "examples" / "python"))
from analyze import analyze, load_library


def iou(a, b):
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    width = max(0, min(ax + aw, bx + bw) - max(ax, bx))
    height = max(0, min(ay + ah, by + bh) - max(ay, by))
    intersection = width * height
    return intersection / (aw * ah + bw * bh - intersection)


def match(expected, predicted, threshold):
    """Find a maximum-cardinality bipartite matching above threshold."""
    edges = [
        sorted(
            ((iou(target["box"], box), j) for j, (box, _) in enumerate(predicted)),
            reverse=True,
        )
        for target in expected
    ]
    assignment = {}

    def augment(i, seen):
        for overlap, j in edges[i]:
            if overlap < threshold or j in seen:
                continue
            seen.add(j)
            if j not in assignment or augment(assignment[j][0], seen):
                assignment[j] = i, overlap
                return True
        return False

    for i in range(len(expected)):
        augment(i, set())
    return assignment


def measure(library, dataset, fixture, fps, threshold):
    clip = dataset / "media" / fixture["clip"]
    width, height = fixture["size"]
    with tempfile.TemporaryDirectory() as scratch:
        rgb = pathlib.Path(scratch) / "frames.rgb"
        subprocess.run(
            ["ffmpeg", "-v", "error", "-i", str(clip), "-an",
             "-vf", f"fps={fps}", "-pix_fmt", "rgb24", "-f", "rawvideo", str(rgb)],
            check=True,
        )
        result = json.loads(analyze(library, rgb, width, height, 0, 1, fps))
    frames = {frame["timestamp"]["pts"]: frame["regions"] for frame in result["observations"]}
    counts = collections.Counter()
    for sample in fixture["samples"]:
        expected = sample["tiles"]
        predicted = []
        for region in frames[sample["tick"]]:
            if region["kind"] == "screen_share":
                continue
            rect = region["rect"]
            box = (rect["x"] * width, rect["y"] * height,
                   rect["width"] * width, rect["height"] * height)
            predicted.append((box, region["kind"]))
        matched = match(expected, predicted, threshold)
        counts.update(
            frames=1,
            expected=len(expected),
            predicted=len(predicted),
            matched=len(matched),
            iou_sum=sum(overlap for _, overlap in matched.values()),
            kind_matches=sum(expected[i]["kind"] == predicted[j][1]
                             for j, (i, _) in matched.items()),
        )
    return counts


def summarize(counts):
    matched = counts["matched"]
    return {
        "frames": counts["frames"],
        "expected": counts["expected"],
        "predicted": counts["predicted"],
        "matched": matched,
        "false_positives": counts["predicted"] - matched,
        "false_negatives": counts["expected"] - matched,
        "precision": matched / counts["predicted"] if counts["predicted"] else None,
        "recall": matched / counts["expected"] if counts["expected"] else None,
        "mean_matched_iou": counts["iou_sum"] / matched if matched else None,
        "matched_kind_accuracy": counts["kind_matches"] / matched if matched else None,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=pathlib.Path)
    parser.add_argument("--iou", type=float, default=0.5)
    parser.add_argument("--dataset", type=pathlib.Path, default=HERE,
                        help="directory containing labels.json and media/")
    args = parser.parse_args()
    if not 0 < args.iou <= 1:
        parser.error("--iou must be greater than 0 and at most 1")
    library = load_library(args.library)
    labels = json.loads((args.dataset / "labels.json").read_text(encoding="utf-8"))
    by_clip = {}
    total = collections.Counter()
    for fixture in labels["fixtures"]:
        counts = measure(library, args.dataset, fixture, labels["frames_per_second"], args.iou)
        by_clip[fixture["clip"]] = summarize(counts)
        # Counter addition discards zero-valued entries, which summarize handles.
        total += counts
    print(json.dumps({
        "iou_threshold": args.iou,
        "clips": by_clip,
        "total": summarize(total),
    }, indent=2))


if __name__ == "__main__":
    main()
