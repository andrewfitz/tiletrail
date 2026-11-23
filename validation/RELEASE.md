# TileTrail 0.1.0 verification

The verified source is `f1cdfb6292f7860ee7a3ca46c9c64e25fe480d7b`. The public C ABI and serialized result schema are both version 1. Later documentation commits record measurements against this source.

## Portability

| Runtime used | Tested entry points |
| --- | --- |
| macOS, Apple Silicon | Rust CPU and Metal, C dynamic library, Swift static archive, Python, independent Rust project using the packaged crate |
| Linux ARM64 container on ARM64 host | Rust and independently linked C CPU consumers |
| Linux x86-64 container with instruction translation | Rust and independently linked C CPU consumers |
| Windows x86-64 GNU executables under Wine 10 | Rust and independently linked C CPU consumers |

All eight Rust/C CPU runs produced identical gallery JSON. The 52-frame fixture gave 11 tracks, 16 appearances and 31 events. The output SHA-256 was `416a352ebfab68e0c1928abb3e6f8cde3b9b4a62b0e328e4952450f7ea83d8c2`. Python and Swift agreed with Rust. Native Windows/MSVC and the hosted CI workflows remain unverified for this source. [CI checks and commands](../.github/workflows)

On Apple Silicon, Rust 1.87.0 passed 52 CPU-only checks and 53 all-feature checks, including integration tests and doctests. The Metal suite exercised device destruction, automatic CPU recovery and strict GPU-required behavior. CPU and Metal produced matching final analyses across the generated fixtures and the independent reviewer's development and stress inputs.

## Picture, identity and timing

The final detector matched all 949 boxes in six generated clips. Two returns and eight expected confirmed events remained unresolved. On the first independent recording set it matched four of 20 labeled boxes, with 12 extra boxes. On two further sessions reserved for final verification it matched two of four, with two extras. In both reserved sets, the Meet webcam was found and the Teams tiles were missed. The real temporal tests separately check identities, event timing and uncertainty. [Synthetic score](../benchmarks/results/gallery-accuracy-2026-11-23.md) · [Real geometry and recording licenses](real/README.md) · [Real event results](temporal/README.md)

## Speed and memory

The same initial and final release binaries were compared over five alternating before/after pairs per workload on an Apple M4 Max. The raw results include binary hashes, input hashes, CPU time and peak RSS. Source-specific experiments measure snapshot memory and GPU workspace reuse. Repeated ambiguous avatars can produce substantial provisional track history. [Paired overall results](../benchmarks/results/end-to-end-final-2026-11-23.md) · [C snapshot memory](../benchmarks/results/snapshot-performance-2026-11-23.md) · [Repeated-avatar stress](../benchmarks/results/follow-on-runtime-2026-11-23.md)

## Local build artifacts

| Artifact | SHA-256 |
| --- | --- |
| Cargo source package | `45d089288a629843d9a45c2256e7871d29687384ddc0fee83ee2523cf7b77e7c` |
| Apple Silicon headers and libraries, compressed tar | `5d8204b7bcbd3a022f711905d189da6ec90359036718a0622c03a6386d951538` |
| Static library linked by MPCut | `7581e5fa05e8908ee52584f4f68b6d89eb630c198f02d6b38fd5d6f6c69c1c6e` |
