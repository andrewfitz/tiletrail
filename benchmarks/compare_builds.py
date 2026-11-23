"""Interleave two release binaries on identical RGB input and save their measurements."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import statistics
import subprocess
import sys


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def measure(executable, mode, fixture, loops):
    mac = sys.platform == "darwin"
    command = ["/usr/bin/time", "-l" if mac else "-v", str(executable), mode,
               str(fixture), "960", "540", "10", str(loops)]
    result = subprocess.run(command, capture_output=True, text=True, check=True)
    sample = json.loads(result.stdout)
    pattern = r"(\d+)\s+maximum resident set size" if mac else r"Maximum resident set size \(kbytes\):\s*(\d+)"
    sample["max_rss_bytes"] = int(re.search(pattern, result.stderr).group(1)) * (1 if mac else 1024)
    return sample


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("fixtures", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--loops", type=int, default=5)
    parser.add_argument("--modes", nargs="+", choices=["cpu", "gpu"], default=["cpu", "gpu"])
    args = parser.parse_args()
    if min(args.loops, args.repetitions) < 1:
        parser.error("loops and repetitions must be positive")
    binaries = {"before": args.before.resolve(), "after": args.after.resolve()}
    report = {"platform": platform.platform(), "loops": args.loops, "binary_sha256": {
        name: sha256(binary) for name, binary in binaries.items()}, "cases": {}}
    for mode in args.modes:
        for name in ("interview", "gallery", "stage_and_share", "speech_control"):
            fixture = args.fixtures / (name + ".rgb")
            samples = {"before": [], "after": []}
            for run in range(args.repetitions):
                for label in (("before", "after") if run % 2 == 0 else ("after", "before")):
                    samples[label].append(measure(binaries[label], mode, fixture, args.loops))
            case = {"input_sha256": sha256(fixture), "samples": samples}
            for label, runs in samples.items():
                case[label] = {metric: statistics.median(run[metric] for run in runs)
                               for metric in ("elapsed_seconds", "max_rss_bytes", "frames_per_second")}
            report["cases"][f"{mode}/{name}"] = case
            print(mode, name, case["before"], case["after"], flush=True)
            args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
