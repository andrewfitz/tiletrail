"""Score automatic feed geometry on composites of four Xiph camera sequences."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "examples" / "python"))
from analyze import analyze, load_library

CLIPS = {"coastguard": "coastguard_qcif", "bus": "bus_qcif_15fps",
         "city": "city_qcif_15fps", "foreman": "foreman_qcif"}
# Each known rectangle is (x, y, width, height) in output pixels.
CASES = {
    "quad": [(0, 0, 176, 144), (184, 0, 176, 144), (0, 152, 176, 144), (184, 152, 176, 144)],
    "borderless": [(0, 0, 176, 144), (176, 0, 176, 144), (0, 144, 176, 144), (176, 144, 176, 144)],
    "asymmetric": [(0, 0, 352, 296), (360, 0, 176, 144), (360, 152, 176, 144)],
    "single": [(0, 0, 176, 144)],
    "pair-swap": [(0, 0, 176, 144), (184, 0, 176, 144)],
    "inset": [(0, 0, 360, 296), (230, 176, 100, 80)],
}


def camera_files(cache):
    for name, stem in CLIPS.items():
        dest = cache / (name + ".y4m")
        if dest.exists() and dest.stat().st_size == 2**21:
            continue
        url = f"https://media.xiph.org/video/derf/y4m/{stem}.y4m"
        request = urllib.request.Request(url, headers={"Range": "bytes=0-2097151"})
        with urllib.request.urlopen(request, timeout=60) as response:
            data = response.read(2**21)
        if len(data) != 2**21 or not data.startswith(b"YUV4MPEG2 "):
            raise ValueError(f"Incomplete Y4M from {url}")
        dest.write_bytes(data)


def composite(cache, name, boxes):
    inputs = list(CLIPS)[:len(boxes)] if len(boxes) > 1 else ["bus"]
    command = ["ffmpeg", "-nostdin", "-y", "-v", "error"]
    for source in inputs:
        command.extend(["-i", str(cache / (source + ".y4m"))])
    graph = [f"[{i}:v]fps=8,setsar=1,scale={w}:{h}[v{i}]" for i, (_, _, w, h) in enumerate(boxes)]
    if name == "inset":
        graph += ["[v1]pad=112:92:6:6:0x8f28ab[inset]", "[v0][inset]overlay=224:170[out]"]
    elif name == "pair-swap":
        graph += ["[v0]pad=360:144:0:0:0x883399[canvas]", "[canvas][v1]overlay=184:0,split[init][next]",
                  "[init]trim=end_frame=6,setpts=PTS-STARTPTS[first]",
                  "[next]trim=start_frame=6:end_frame=12,setpts=PTS-STARTPTS,split[a][b]",
                  "[a]crop=176:144:0:0[l]", "[b]crop=176:144:184:0,pad=360:144:0:0:0x883399[r]",
                  "[r][l]overlay=184:0[swapped]", "[first][swapped]concat=n=2:v=1:a=0[out]"]
    elif len(boxes) > 1:
        positions = "|".join(f"{x}_{y}" for x, y, _, _ in boxes)
        channels = "".join(f"[v{i}]" for i in range(len(boxes)))
        graph.append(f"{channels}xstack=inputs={len(boxes)}:layout={positions}:fill=black[out]")
    else:
        graph.append("[v0]null[out]")
    movie, raw = cache / (name + ".mp4"), cache / (name + ".rgb")
    subprocess.run(command + ["-filter_complex", ";".join(graph), "-map", "[out]", "-frames:v", "12",
                              "-c:v", "libx264", "-crf", "18", str(movie)], check=True)
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-i", str(movie), "-pix_fmt", "rgb24",
                    "-f", "rawvideo", str(raw)], check=True)
    return raw


def overlap(box, pred):
    x, y, w, h = box
    px, py, pw, ph = (pred[k] for k in ("x", "y", "width", "height"))
    area = max(0, min(x + w, px + pw) - max(x, px)) * max(0, min(y + h, py + ph) - max(y, py))
    return area / (w * h + pw * ph - area)


def score(cache, library, layout):
    scores = {}
    for name, boxes in CASES.items():
        raw = composite(cache, name, boxes)
        width, height = max(x + w for x, _, w, _ in boxes), max(y + h for _, y, _, h in boxes)
        expected = [(x / width, y / height, w / width, h / height) for x, y, w, h in boxes]
        frames = json.loads(analyze(library, raw, width, height, 0, 1, 8, layout))["observations"]
        if len(frames) != 12:
            raise RuntimeError(f"{name}: expected 12 decoded frames")
        exact, ious, identities = 0, [], []
        for frame in frames:
            regions = frame["regions"]
            best = [max((overlap(box, region["rect"]) for region in regions), default=0) for box in expected]
            if name == "pair-swap" and regions:
                identities.append([max(regions, key=lambda region: overlap(box, region["rect"]))["participant_id"]
                                   for box in expected])
            ious.extend(best)
            exact += len(boxes) == len(regions) and min(best) >= 0.90
        scores[name] = {"frames": len(frames), "correct_layout_frames": exact,
                        "mean_best_iou": round(sum(ious) / len(ious), 4)}
        if name == "pair-swap":
            first = identities[0] if identities else []
            scores[name]["identities_followed_swap"] = (
                len(identities) == 12 and len(set(first)) == 2 and None not in first
                and all(ids == (first if t < 6 else first[::-1]) for t, ids in enumerate(identities))
            )
    return scores


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("--cache", type=Path, default=ROOT / "target" / "feed-validation")
    parser.add_argument("--meeting", action="store_true", help="measure the original meeting detector")
    args = parser.parse_args()
    args.cache.mkdir(parents=True, exist_ok=True)
    camera_files(args.cache)
    print(json.dumps(score(args.cache, load_library(args.library), None if args.meeting else (0, 0)), indent=2))
