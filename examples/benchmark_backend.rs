//! Time TileTrail analysis over caller-owned RGB and luma buffers.
//!
//! Generate raw 960x540 RGB fixtures with benchmarks/make_gallery.py,
//! then run: cargo run --release --features gpu --example benchmark_backend --
//! path/to/gallery.rgb

use std::{env, error::Error, fs, hint::black_box, time::Instant};

use tiletrail::{Analyzer, AnalyzerConfig, BackendMode, Frame, PixelFormat, Sampling, Timestamp};

const INPUT_WIDTH: usize = 960;
const INPUT_HEIGHT: usize = 540;
const INPUT_FRAME_BYTES: usize = INPUT_WIDTH * INPUT_HEIGHT * 3;
const MAX_INPUT_FRAMES: usize = 12;
const SCANS: usize = 6;

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args_os()
        .nth(1)
        .ok_or("provide a raw 960x540 rgb24 fixture path")?;
    let input = fs::read(path)?;
    if input.is_empty() || input.len() % INPUT_FRAME_BYTES != 0 {
        return Err("fixture must contain complete 960x540 rgb24 frames".into());
    }
    let source_frames = input.len() / INPUT_FRAME_BYTES;
    let sample_count = source_frames.min(MAX_INPUT_FRAMES);
    for scale in [1, 2] {
        let width = INPUT_WIDTH * scale;
        let height = INPUT_HEIGHT * scale;
        for format in [PixelFormat::Rgb24, PixelFormat::Gray8] {
            let frames = (0..sample_count)
                .map(|sample| {
                    let index = sample * (source_frames - 1) / (sample_count - 1).max(1);
                    let offset = index * INPUT_FRAME_BYTES;
                    let rgb = &input[offset..offset + INPUT_FRAME_BYTES];
                    (index, prepare_frame(rgb, width, height, scale, format))
                })
                .collect::<Vec<_>>();
            for mode in [BackendMode::CpuOnly, BackendMode::GpuRequired] {
                benchmark(&frames, width, height, format, mode)?;
            }
        }
    }
    Ok(())
}

fn prepare_frame(
    rgb: &[u8],
    width: usize,
    height: usize,
    scale: usize,
    format: PixelFormat,
) -> Vec<u8> {
    let mut prepared = Vec::with_capacity(width * height * format.bytes_per_pixel());
    for y in 0..height {
        for x in 0..width {
            let index = (y / scale * INPUT_WIDTH + x / scale) * 3;
            let pixel = &rgb[index..index + 3];
            if format == PixelFormat::Gray8 {
                let luma = (77 * u16::from(pixel[0])
                    + 150 * u16::from(pixel[1])
                    + 29 * u16::from(pixel[2]))
                    >> 8;
                prepared.push(luma as u8);
            } else {
                prepared.extend_from_slice(pixel);
            }
        }
    }
    prepared
}

fn benchmark(
    frames: &[(usize, Vec<u8>)],
    width: usize,
    height: usize,
    format: PixelFormat,
    mode: BackendMode,
) -> Result<(), Box<dyn Error>> {
    let mut setup_ms = Vec::with_capacity(SCANS - 1);
    let mut analysis_ms = Vec::with_capacity(SCANS - 1);
    let mut summary = (0, 0);
    for scan in 0..SCANS {
        let clock = |index: usize| Timestamp::new(index as i64, 1, 10);
        let setup_start = Instant::now();
        let mut analyzer = Analyzer::new(
            AnalyzerConfig {
                mode,
                sampling: Sampling::Sparse,
            },
            clock(0),
        )?;
        let analysis_start = Instant::now();
        for (index, frame) in frames {
            analyzer.push(Frame {
                data: frame,
                width: width as u32,
                height: height as u32,
                stride: width * format.bytes_per_pixel(),
                format,
                timestamp: clock(*index),
                display: Default::default(),
            })?;
        }
        let result = black_box(analyzer.finish(clock(frames.last().unwrap().0 + 1))?);
        let elapsed_ms = analysis_start.elapsed().as_secs_f64() * 1000.0;
        if scan > 0 {
            setup_ms.push((analysis_start - setup_start).as_secs_f64() * 1000.0);
            analysis_ms.push(elapsed_ms);
        }
        summary = (result.tracks.len(), result.events.len());
    }
    setup_ms.sort_by(f64::total_cmp);
    analysis_ms.sort_by(f64::total_cmp);
    let frames_per_scan = frames.len();
    let median_setup = setup_ms[setup_ms.len() / 2];
    let median_scan = analysis_ms[analysis_ms.len() / 2];
    println!(
        "{width}x{height} {format:?} {mode:?}  {frames_per_scan} frames  init {median_setup:.2} ms  analysis {:.3} ms/frame  {} tracks, {} events",
        median_scan / frames_per_scan as f64,
        summary.0,
        summary.1,
    );
    Ok(())
}
