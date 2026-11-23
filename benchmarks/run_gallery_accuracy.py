#!/usr/bin/env python3
"""Score frozen CPU and Metal release analyses against all six synthetic clips."""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess

from make_gallery import HELD_OUT, SCENARIOS
from score_gallery import score


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def analyze(executable, fixture, truth, output, backend):
    subprocess.run(
        [str(executable), backend, str(fixture), str(truth["width"]), str(truth["height"]),
         str(truth["fps"]), "1", str(output)],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    result = json.loads(output.read_text())
    if result["diagnostics"][-1]["backend"] != backend:
        raise ValueError(f"{fixture.name} ran on an unexpected backend")
    return result


def compare_backends(cpu, metal):
    payload = {key for key in cpu.keys() | metal.keys() if key != "diagnostics"}
    mismatches = sorted(key for key in payload if cpu.get(key) != metal.get(key))
    return {
        "equal_except_backend_diagnostics": not mismatches,
        "different_fields": mismatches,
        "observations": len(cpu["observations"]),
        "tracks": len(cpu["tracks"]),
        "appearances": len(cpu["appearances"]),
        "events": len(cpu["events"]),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path, help="release gallery_benchmark built with gpu")
    parser.add_argument("fixtures", type=Path)
    parser.add_argument("raw_results", type=Path, help="directory for backend analysis JSON")
    parser.add_argument("report", type=Path)
    parser.add_argument("--baseline", type=Path,
                        default=Path(__file__).parent / "results" / "gallery-initial-2026-11-23.json")
    parser.add_argument("--baseline-raw", type=Path, help="original CPU analysis JSON directory")
    parser.add_argument("--reference", type=Path, help="score report from an earlier freeze")
    parser.add_argument("--reference-raw", type=Path, help="CPU/Metal analyses for the earlier freeze")
    parser.add_argument("--revision", required=True, help="source revision of the executable")
    args = parser.parse_args()
    if bool(args.reference) != bool(args.reference_raw):
        parser.error("--reference and --reference-raw must be used together")
    initial = json.loads(args.baseline.read_text())
    reference = json.loads(args.reference.read_text()) if args.reference else None
    executable = args.executable.resolve()
    args.raw_results.mkdir(exist_ok=True, parents=True)
    report = {
        "revision": args.revision,
        "platform": platform.platform(),
        "release_executable_sha256": sha256(executable),
        "baseline_executable_sha256": initial["initial_cpu_executable_sha256"],
        "cases": {},
    }
    if reference is not None:
        report["reference_revision"] = reference["revision"]
        report["reference_executable_sha256"] = reference["release_executable_sha256"]
    for name in SCENARIOS | HELD_OUT:
        fixture = args.fixtures / f"{name}.rgb"
        annotation = args.fixtures / f"{name}.truth.json"
        truth = json.loads(annotation.read_text())
        input_sha256 = sha256(fixture)
        annotation_sha256 = sha256(annotation)
        initial_sha256 = initial["fixtures_rgb24_sha256"].get(name)
        if initial_sha256 is not None and initial_sha256 != input_sha256:
            raise ValueError(f"{name} fixture does not match the initial sample")
        if reference is not None:
            prior_case = reference["cases"][name]
            if (input_sha256, annotation_sha256) != (
                prior_case["input_rgb24_sha256"], prior_case["annotation_sha256"]
            ):
                raise ValueError(f"{name} input or annotation does not match the reference")
        initial_score = initial["accuracy"].get(name)
        if args.baseline_raw is not None and initial_score is not None:
            initial_analysis = json.loads((args.baseline_raw / f"{name}.analysis.json").read_text())
            rescored = score(truth, initial_analysis)
            changed = [key for key in initial_score if rescored.get(key) != initial_score[key]]
            if changed:
                raise ValueError(f"{name} baseline rescore changed recorded fields: {changed}")
            initial_score = rescored
        cpu = analyze(executable, fixture, truth, args.raw_results / f"{name}.cpu.json", "cpu")
        metal = analyze(executable, fixture, truth, args.raw_results / f"{name}.gpu.json", "gpu")
        case = {
            "split": "held_out" if name in HELD_OUT else "development",
            "input_rgb24_sha256": input_sha256,
            "annotation_sha256": annotation_sha256,
            "initial_cpu_score": initial_score,
            "cpu_score": score(truth, cpu),
            "metal_score": score(truth, metal),
            "backend_parity": compare_backends(cpu, metal),
        }
        if reference is not None:
            prior_cpu = json.loads((args.reference_raw / f"{name}.cpu.json").read_text())
            prior_metal = json.loads((args.reference_raw / f"{name}.gpu.json").read_text())
            case["freeze_parity"] = {
                "cpu": compare_backends(prior_cpu, cpu),
                "metal": compare_backends(prior_metal, metal),
            }
        report["cases"][name] = case
    args.report.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
