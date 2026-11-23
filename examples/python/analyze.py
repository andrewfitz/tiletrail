"""Analyze packed RGB24 frames through the TileTrail shared library."""

import argparse
import ctypes as c
from pathlib import Path
import sys


class Timestamp(c.Structure):
    _fields_ = [
        ("pts", c.c_int64), ("numerator", c.c_uint32), ("denominator", c.c_uint32)
    ]


class Config(c.Structure):
    _fields_ = [("mode", c.c_uint32), ("sampling", c.c_uint32)]


class Frame(c.Structure):
    _fields_ = [
        ("data", c.POINTER(c.c_uint8)),
        ("len", c.c_uint64),
        ("stride", c.c_uint64),
        ("width", c.c_uint32),
        ("height", c.c_uint32),
        ("format", c.c_uint32),
        ("rotation", c.c_uint32),
        ("pixel_aspect_num", c.c_uint32),
        ("pixel_aspect_den", c.c_uint32),
        ("timestamp", Timestamp),
    ]


class Error(c.Structure):
    _fields_ = [
        ("code", c.c_int32),
        ("has_resume", c.c_uint32),
        ("resume_at", Timestamp),
        ("message", c.c_char * 256),
    ]


class Buffer(c.Structure):
    _fields_ = [("data", c.POINTER(c.c_uint8)), ("len", c.c_uint64)]


def load_library(path):
    library = c.CDLL(str(path.resolve()))
    analyzer = c.c_void_p
    error = c.POINTER(Error)
    signatures = {
        "tiletrail_abi_version": (c.c_uint32, []),
        "tiletrail_analyzer_create": (
            c.c_int32,
            [c.POINTER(Config), Timestamp, c.POINTER(analyzer), error],
        ),
        "tiletrail_analyzer_push": (
            c.c_int32,
            [analyzer, c.POINTER(Frame), error],
        ),
        "tiletrail_analyzer_result_json": (
            c.c_int32,
            [analyzer, Timestamp, c.POINTER(Buffer), error],
        ),
        "tiletrail_analyzer_free": (None, [analyzer]),
        "tiletrail_buffer_free": (None, [c.POINTER(Buffer)]),
    }
    for name, (result, arguments) in signatures.items():
        function = getattr(library, name)
        function.restype = result
        function.argtypes = arguments
    if library.tiletrail_abi_version() != 1:
        raise RuntimeError("unsupported TileTrail C ABI version")
    return library


def check(status, error):
    if status != 0:
        message = error.message.decode("utf-8", errors="replace")
        if error.has_resume:
            time = error.resume_at
            message += f" (resume at {time.pts} ticks of {time.numerator}/{time.denominator} s)"
        raise RuntimeError(f"TileTrail error {status}: {message}")


def analyze(library, source, width, height, first_pts, numerator, denominator):
    if not all(0 < value < 2**32 for value in (width, height, numerator, denominator)):
        raise ValueError("dimensions and time base fields must be positive uint32 values")
    if not -(2**63) <= first_pts < 2**63:
        raise ValueError("first PTS must fit an int64 value")
    stride = width * 3
    length = stride * height
    if length > sys.maxsize:
        raise ValueError("frame size exceeds addressable range")

    time = Timestamp(first_pts, numerator, denominator)
    config = Config(1, 0)  # CPU-only, every frame
    analyzer, error, output = c.c_void_p(), Error(), Buffer()
    check(library.tiletrail_analyzer_create(c.byref(config), time, c.byref(analyzer), c.byref(error)),
          error)
    try:
        pixels = bytearray(length)
        data = (c.c_uint8 * length).from_buffer(pixels)
        with source.open("rb") as stream:
            while count := stream.readinto(pixels):
                if count != length or time.pts == 2**63 - 1:
                    raise ValueError("truncated frame or timestamp overflow")
                frame = Frame(data, length, stride, width, height, 1, 0, 1, 1, time)  # RGB24
                check(library.tiletrail_analyzer_push(analyzer, c.byref(frame), c.byref(error)),
                      error)
                time.pts += 1
        check(library.tiletrail_analyzer_result_json(analyzer, time, c.byref(output), c.byref(error)),
              error)
        return c.string_at(output.data, output.len)
    finally:
        library.tiletrail_buffer_free(c.byref(output))
        library.tiletrail_analyzer_free(analyzer)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("frames", type=Path)
    for arg in ("width", "height", "first_pts", "tick_numerator", "tick_denominator"):
        parser.add_argument(arg, type=int)
    args = parser.parse_args()
    library = load_library(args.library)
    result = analyze(library, args.frames, args.width, args.height, args.first_pts,
                     args.tick_numerator, args.tick_denominator)
    sys.stdout.buffer.write(result + b"\n")


if __name__ == "__main__":
    main()
