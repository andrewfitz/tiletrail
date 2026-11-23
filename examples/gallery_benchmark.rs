use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::time::Instant;

use tiletrail::{Analyzer, AnalyzerConfig, BackendMode, Frame, PixelFormat, Sampling, Timestamp};

const USAGE: &str =
    "usage: gallery_benchmark CPU_OR_GPU FRAMES.rgb WIDTH HEIGHT FPS LOOPS [RESULT.json]";

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [mode, input, width, height, fps, loops, rest @ ..] = args.as_slice() else {
        return Err(USAGE.into());
    };
    if rest.len() > 1 {
        return Err(USAGE.into());
    }
    let mode = match mode.to_str() {
        Some("cpu") => BackendMode::CpuOnly,
        Some("gpu") => BackendMode::GpuRequired,
        _ => return Err(USAGE.into()),
    };
    let parse = |value: &std::ffi::OsStr| -> Result<u32, Box<dyn Error>> {
        let value: u32 = value.to_str().ok_or(USAGE)?.parse()?;
        if value == 0 {
            return Err(USAGE.into());
        }
        Ok(value)
    };
    let (width, height, fps, loops) = (parse(width)?, parse(height)?, parse(fps)?, parse(loops)?);
    let stride = (width as usize).checked_mul(3).ok_or("stride overflow")?;
    let frame_bytes = stride
        .checked_mul(height as usize)
        .ok_or("frame overflow")?;
    let size = std::fs::metadata(input)?.len();
    if size == 0 || size % frame_bytes as u64 != 0 {
        return Err("input must contain a whole number of packed RGB24 frames".into());
    }
    let frames_per_loop = size / frame_bytes as u64;
    frames_per_loop
        .checked_mul(u64::from(loops))
        .filter(|&frames| i64::try_from(frames).is_ok())
        .ok_or("frame count exceeds timestamp range")?;
    let mut buffer = vec![0; frame_bytes];
    let mut committed = 0;
    let start = Instant::now();
    let mut analyzer = Analyzer::new(
        AnalyzerConfig {
            mode,
            sampling: Sampling::EveryFrame,
        },
        Timestamp::new(0, 1, fps),
    )?;
    let initialized = Instant::now();
    for _ in 0..loops {
        let mut source = File::open(input)?;
        for _ in 0..frames_per_loop {
            source.read_exact(&mut buffer)?;
            analyzer.push(Frame {
                data: &buffer,
                width,
                height,
                stride,
                format: PixelFormat::Rgb24,
                timestamp: Timestamp::new(committed, 1, fps),
                display: Default::default(),
            })?;
            committed += 1;
        }
    }
    let pushed = Instant::now();
    let result = analyzer.snapshot(Timestamp::new(committed, 1, fps))?;
    let finished = Instant::now();
    let elapsed = (finished - start).as_secs_f64();
    println!(
        "{}",
        serde_json::json!({
            "backend": analyzer.backend(),
            "committed_frames": committed,
            "initialization_seconds": (initialized - start).as_secs_f64(),
            "read_and_push_seconds": (pushed - initialized).as_secs_f64(),
            "finish_seconds": (finished - pushed).as_secs_f64(),
            "elapsed_seconds": elapsed,
            "frames_per_second": committed as f64 / elapsed,
            "realtime_multiple": committed as f64 / elapsed / f64::from(fps),
            "tracks": result.tracks.len(),
            "events": result.events.len(),
        })
    );
    if let Some(path) = rest.first() {
        let mut output = BufWriter::new(File::create(path)?);
        serde_json::to_writer(&mut output, &result)?;
        output.flush()?;
    }
    Ok(())
}
