"""Check the source clock and compare gallery snapshots across consumers."""

import argparse
import json
import math
from pathlib import Path


def check_result(result, backend):
    ticks = {"numerator": 1, "denominator": 10}
    assert result["schema_version"] == 1
    assert result["start"] == {"pts": 0, "time_base": ticks}
    assert result["end"] == {"pts": 52, "time_base": ticks}
    assert result["resume_at"] is None
    assert result["diagnostics"][-1]["backend"] == backend
    assert [frame["timestamp"] for frame in result["observations"]] == [
        {"pts": pts, "time_base": ticks} for pts in range(52)
    ]
    assert len(result["observations"][0]["regions"]) == 9
    assert result["tracks"] and result["events"]


def compare_metal(cpu, gpu):
    # Region geometry is in normalized display coordinates. One source pixel
    # in either axis is the maximum accepted difference for the 960x540 input.
    tolerance = {"x": 1 / 960, "y": 1 / 540, "width": 1 / 960, "height": 1 / 540}
    for key in ("start", "end", "coverage", "tracks", "appearances", "events"):
        assert cpu[key] == gpu[key], f"Metal disagrees on {key}"
    for cpu_frame, gpu_frame in zip(cpu["observations"], gpu["observations"]):
        cpu_regions = cpu_frame["regions"]
        gpu_regions = gpu_frame["regions"]
        assert len(cpu_regions) == len(gpu_regions)
        for expected, measured in zip(cpu_regions, gpu_regions):
            for key in ("participant_id", "appearance_id", "role", "kind"):
                assert expected[key] == measured[key], f"Metal disagrees on {key}"
            for axis in ("x", "y", "width", "height"):
                assert math.isclose(expected["rect"][axis], measured["rect"][axis],
                                    rel_tol=0, abs_tol=tolerance[axis]), f"Metal disagrees on {axis}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metal", action="store_true")
    parser.add_argument("output", type=Path, nargs="+")
    args = parser.parse_args()
    cpu, *others = [json.loads(path.read_bytes()) for path in args.output]
    check_result(cpu, "cpu")
    for result in others:
        if args.metal:
            check_result(result, "gpu")
            compare_metal(cpu, result)
        else:
            assert cpu == result, "CPU consumers returned different results"


if __name__ == "__main__":
    main()
