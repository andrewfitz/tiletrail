"""Measure repeated Metal workspace resizing through the public TileTrail ABI."""
import argparse
import ctypes as c
import hashlib
import json
from pathlib import Path
import resource
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "examples" / "python"))
from analyze import Buffer, Config, Error, Frame, Timestamp, check, load_library


def measure(library_path, count):
    library = load_library(library_path)
    frames = []
    for width, height in [(960, 540), (256, 256)]:
        stride = 3 * width
        data = (c.c_uint8 * (stride * height))()
        frames.append(Frame(data, len(data), stride, width, height, 1, 0, 1, 1, Timestamp(0, 1, 10)))

    config = Config(2, 0)  # GPU required, every frame
    analyzer, error, output = c.c_void_p(), Error(), Buffer()
    check(library.tiletrail_analyzer_create(c.byref(config), Timestamp(0, 1, 10),
                                           c.byref(analyzer), c.byref(error)), error)
    try:
        start, cpu_start = time.perf_counter(), time.process_time()
        for index in range(count):
            frame = frames[index % len(frames)]
            frame.timestamp = Timestamp(index, 1, 10)
            check(library.tiletrail_analyzer_push(analyzer, c.byref(frame), c.byref(error)), error)
        wall, cpu = time.perf_counter() - start, time.process_time() - cpu_start
        check(library.tiletrail_analyzer_result_json(analyzer, Timestamp(count, 1, 10),
                                                    c.byref(output), c.byref(error)), error)
        digest = hashlib.sha256(c.string_at(output.data, output.len)).hexdigest()
    finally:
        library.tiletrail_buffer_free(c.byref(output))
        library.tiletrail_analyzer_free(analyzer)
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return {"wall": wall, "cpu": cpu, "rss": rss * (1 if sys.platform == "darwin" else 1024),
            "digest": digest}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("--frames", type=int, default=200)
    args = parser.parse_args()
    if not 1 <= args.frames < 2**63:
        parser.error("frames must be a positive int64 count")
    print(json.dumps(measure(args.library.resolve(), args.frames)))
