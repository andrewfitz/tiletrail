mod media;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use tiletrail::*;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const HELP: &str =
    "tiletrail scan INPUT [--output SCAN] [--stream INDEX] [--sampling every-frame|sparse]
                     [--gallery-grid COLSxROWS | --feed-layout auto|COLSxROWS]
tiletrail inspect SCAN [--at TIME | --range START END | --layout-changes | --issues | --feed-id ID]

Scan uses ffmpeg and ffprobe. Times are local to the recording, in seconds,
HH:MM:SS.sss, or numerator/denominator. Reports are JSON.";

#[derive(Clone, Copy, Serialize)]
struct Rational {
    numerator: i64,
    denominator: i64,
}

impl Rational {
    fn from_ticks(ticks: i64, base: TimeBase) -> Result<Self> {
        Ok(Self::new(
            ticks
                .checked_mul(i64::from(base.numerator))
                .ok_or("source time overflow")?,
            i64::from(base.denominator),
        ))
    }

    fn new(numerator: i64, denominator: i64) -> Self {
        let (mut a, mut b) = (numerator.unsigned_abs(), denominator as u64);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Self {
            numerator: numerator / a as i64,
            denominator: denominator / a as i64,
        }
    }
}

#[derive(Serialize)]
struct ScanSource {
    producer: &'static str,
    file: PathBuf,
    stamp: media::Stamp,
    content_sha256: String,
    stream: u32,
    origin: Rational,
    time_base: Rational,
    duration: Rational,
    transform: DisplayGeometry,
}

#[derive(Serialize)]
struct ScanConfiguration {
    backend: &'static str,
    sampling: Sampling,
    include_scenes: bool,
    feed_layout: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sparse_interval: Option<Rational>,
}

#[derive(Serialize)]
struct Scan {
    state: &'static str,
    source: ScanSource,
    configuration: ScanConfiguration,
    analysis: AnalysisResult,
    analysis_identity: String,
}

fn dimensions(value: &str) -> Result<(u32, u32)> {
    let (columns, rows) = value
        .split_once('x')
        .ok_or("expected dimensions COLSxROWS")?;
    let (columns, rows): (u32, u32) = (columns.parse()?, rows.parse()?);
    if !matches!(columns.checked_mul(rows), Some(1..=MAX_GRID_FEEDS)) {
        return Err("grid dimensions must describe between 1 and 64 feeds".into());
    }
    Ok((columns, rows))
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    match args.next() {
        Some(value) if !value.starts_with("--") => Ok(value),
        _ => Err(format!("missing value for {flag}").into()),
    }
}

fn scan(file: PathBuf, mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut output = None;
    let mut stream = None;
    let mut layout = FeedLayout::Meeting;
    let mut sampling = Sampling::EveryFrame;
    while let Some(flag) = args.next() {
        let value = next_value(&mut args, &flag)?;
        match flag.as_str() {
            "--output" => output = Some(PathBuf::from(value)),
            "--stream" => stream = Some(value.parse()?),
            "--sampling" => {
                sampling = match value.as_str() {
                    "every-frame" => Sampling::EveryFrame,
                    "sparse" => Sampling::Sparse,
                    _ => return Err("sampling must be every-frame or sparse".into()),
                }
            }
            "--gallery-grid" | "--feed-layout" if layout != FeedLayout::Meeting => {
                return Err("choose one gallery or feed layout".into());
            }
            "--gallery-grid" => {
                let (columns, rows) = dimensions(&value)?;
                layout = FeedLayout::Gallery { columns, rows };
            }
            "--feed-layout" => {
                layout = if value == "auto" {
                    FeedLayout::Automatic
                } else {
                    let (columns, rows) = dimensions(&value)?;
                    FeedLayout::Grid { columns, rows }
                };
            }
            _ => return Err(format!("unknown option {flag}").into()),
        }
    }
    let file = fs::canonicalize(file)?;
    let stamp = media::stamp(&file)?;
    if let Some(path) = &output
        && path.try_exists()?
    {
        return Err("output already exists".into());
    }
    let content_sha256 = media::digest(&file)?;
    let media = media::Media::open(&file, stream)?;
    let time = |pts| Timestamp {
        pts,
        time_base: media.time_base,
    };
    let mut analyzer = Analyzer::with_layout(
        AnalyzerConfig {
            mode: BackendMode::CpuOnly,
            sampling,
        },
        time(0),
        layout,
    )?;
    let mut last_sample = None::<Timestamp>;
    // The same half-second cadence MPCut uses for Sparse.
    let sparse_interval = Rational::new(1, 2);
    media.decode(&file, |timestamp, pixels| {
        if sampling == Sampling::Sparse
            && let Some(last) = last_sample
            && (i128::from(timestamp.pts) - i128::from(last.pts))
                * i128::from(timestamp.time_base.numerator)
                * i128::from(sparse_interval.denominator)
                < i128::from(timestamp.time_base.denominator)
                    * i128::from(sparse_interval.numerator)
        {
            return Ok(());
        }
        analyzer.push(Frame {
            data: pixels,
            width: media.width,
            height: media.height,
            stride: media.width as usize * 3,
            format: PixelFormat::Rgb24,
            timestamp,
            display: media.display,
        })?;
        last_sample = Some(timestamp);
        Ok(())
    })?;
    if last_sample.is_none() {
        return Err("no decoded frames in the presentation range".into());
    }
    if stamp != media::stamp(&file)?
        || content_sha256 != media::digest(&file)?
        || stamp != media::stamp(&file)?
    {
        return Err("input changed during the scan".into());
    }
    let source = ScanSource {
        producer: "tiletrail",
        file,
        stamp,
        content_sha256,
        stream: media.index,
        origin: Rational::from_ticks(media.origin, media.time_base)?,
        time_base: Rational::from_ticks(1, media.time_base)?,
        duration: Rational::from_ticks(media.duration, media.time_base)?,
        transform: media.display,
    };
    let configuration = ScanConfiguration {
        backend: "cpu",
        sampling,
        include_scenes: false,
        feed_layout: match layout {
            FeedLayout::Meeting => "meeting".into(),
            FeedLayout::Automatic => "auto".into(),
            FeedLayout::Grid { columns, rows } => format!("{columns}x{rows}"),
            FeedLayout::Gallery { columns, rows } => format!("gallery:{columns}x{rows}"),
        },
        sparse_interval: (sampling == Sampling::Sparse).then_some(sparse_interval),
    };
    let analysis = analyzer.finish(time(media.duration))?;
    let analysis_identity = format!(
        "tiletrail:{}:{}:{}:{}:{:?}:{}",
        env!("CARGO_PKG_VERSION"),
        source.content_sha256,
        source.stream,
        configuration.feed_layout,
        sampling,
        analysis.schema_version
    );
    let report = Scan {
        state: "completed",
        source,
        configuration,
        analysis,
        analysis_identity,
    };
    if let Some(path) = output {
        write_new(&path, &report)
    } else {
        write_json(BufWriter::new(io::stdout().lock()), &report)
    }
}

fn write_json(mut out: impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut out, value)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

fn write_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let out = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = write_json(BufWriter::new(out), value);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn timestamp(value: &str) -> Result<Timestamp> {
    let invalid = "invalid timestamp";
    if let Some((top, bottom)) = value.split_once('/') {
        let bottom: u32 = bottom.parse()?;
        if bottom == 0 {
            return Err(invalid.into());
        }
        return Ok(Timestamp::new(top.parse()?, 1, bottom));
    }
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |s| (true, s));
    let mut clock = value.rsplit(':');
    let seconds = clock.next().ok_or(invalid)?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
        || seconds.ends_with('.')
    {
        return Err(invalid.into());
    }
    let whole: u64 = if whole.is_empty() { 0 } else { whole.parse()? };
    let fraction_value: u64 = if fraction.is_empty() {
        0
    } else {
        fraction.parse()?
    };
    let denominator = 10u32
        .checked_pow(fraction.len().try_into()?)
        .ok_or(invalid)?;
    let minutes = clock.next().map(str::parse::<u64>).transpose()?;
    let hours = clock.next().map(str::parse::<u64>).transpose()?;
    if clock.next().is_some()
        || (whole >= 60 && minutes.is_some())
        || hours.is_some() && minutes.unwrap_or(0) >= 60
    {
        return Err(invalid.into());
    }
    let seconds = i128::from(hours.unwrap_or(0)) * 3600
        + i128::from(minutes.unwrap_or(0)) * 60
        + i128::from(whole);
    let magnitude = seconds * i128::from(denominator) + i128::from(fraction_value);
    let ticks = if negative { -magnitude } else { magnitude };
    Ok(Timestamp::new(ticks.try_into()?, 1, denominator))
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InspectInput {
    Scan { analysis: AnalysisResult },
    Analysis(AnalysisResult),
}

fn issues(analysis: &AnalysisResult) -> Value {
    let mut issues = Vec::<(Timestamp, Value)>::new();
    for interval in &analysis.coverage {
        if interval.status != Coverage::Observed {
            issues.push((
                interval.start,
                json!({
                    "kind": "coverage", "timestamp": interval.start, "interval": interval
                }),
            ));
        }
    }
    for event in &analysis.events {
        if event.uncertain {
            issues.push((
                event.timestamp,
                json!({
                    "kind": "uncertain_change", "timestamp": event.timestamp, "event": event
                }),
            ));
        }
    }
    let mut last_confirmed = HashMap::<FeedId, (Timestamp, &RegionObservation)>::new();
    for observation in &analysis.observations {
        let time = observation.timestamp;
        for region in &observation.regions {
            if region.identity_uncertain
                || (region.participant_id.is_none() && region.kind != TileKind::ScreenShare)
            {
                let evidence: Vec<_> = region.possible_matches.iter().map(|candidate| {
                    let prior = last_confirmed.get(&candidate.participant_id).map(|(time, region)| {
                        json!({"timestamp": time, "appearance_id": region.appearance_id, "rect": region.rect})
                    });
                    json!({"match": candidate, "last_confirmed": prior})
                }).collect();
                issues.push((time, json!({
                    "kind": "unresolved_identity", "timestamp": time, "region": region, "evidence": evidence
                })));
            }
        }
        for region in &observation.regions {
            if !region.identity_uncertain
                && let Some(feed) = region.feed_id()
            {
                last_confirmed.insert(feed, (time, region));
            }
        }
    }
    issues.sort_by(|a, b| a.0.compare(b.0).unwrap_or(std::cmp::Ordering::Equal));
    json!({"issues": issues.into_iter().map(|(_, value)| value).collect::<Vec<_>>()})
}

fn inspect(file: PathBuf, mut args: impl Iterator<Item = String>) -> Result<()> {
    enum Query {
        Summary,
        At(Timestamp),
        Range(Timestamp, Timestamp),
        LayoutChanges,
        Issues,
        Feed(FeedId),
    }
    let query = match args.next().as_deref() {
        None => Query::Summary,
        Some("--issues") => Query::Issues,
        Some("--layout-changes") => Query::LayoutChanges,
        Some("--range") => {
            let start = timestamp(&next_value(&mut args, "--range START")?)?;
            let end = timestamp(&next_value(&mut args, "--range END")?)?;
            if !start.compare(end).is_some_and(|order| order.is_lt()) {
                return Err("range end must follow its start".into());
            }
            Query::Range(start, end)
        }
        Some("--at") => Query::At(timestamp(&next_value(&mut args, "--at")?)?),
        Some("--feed-id") => Query::Feed(next_value(&mut args, "--feed-id")?.parse()?),
        Some(flag) => return Err(format!("unknown option {flag}").into()),
    };
    if args.next().is_some() {
        return Err("choose one inspect query".into());
    }
    let input = serde_json::from_reader(BufReader::new(File::open(file)?))?;
    let analysis = match input {
        InspectInput::Scan { analysis } | InspectInput::Analysis(analysis) => analysis,
    };
    if analysis.schema_version != RESULT_SCHEMA_VERSION {
        return Err("unsupported analysis schema".into());
    }
    let report: Value = match query {
        Query::Summary => json!({
            "start": analysis.start,
            "end": analysis.end,
            "sampling": analysis.sampling,
            "feeds": analysis.tracks.len(),
            "appearances": analysis.appearances.len(),
            "peak_visible": analysis.observations.iter().map(|o| o.regions.len()).max().unwrap_or(0),
            "layout_changes": analysis.events,
        }),
        Query::At(time) => json!({
            "timestamp": time,
            "coverage": analysis.coverage_at(time),
            "observation": analysis.visible_at(time),
        }),
        Query::Range(start, end) => {
            let overlaps = |a: Timestamp, b: Timestamp| {
                a.compare(end).is_some_and(|order| order.is_lt())
                    && start.compare(b).is_some_and(|order| order.is_lt())
            };
            json!({
                "start": start,
                "end": end,
                "appearances": analysis.appearances.iter().filter(|item| overlaps(item.start, item.end)).collect::<Vec<_>>(),
                "coverage": analysis.coverage.iter().filter(|item| overlaps(item.start, item.end)).collect::<Vec<_>>(),
            })
        }
        Query::LayoutChanges => json!({"layout_changes": analysis.events}),
        Query::Issues => issues(&analysis),
        Query::Feed(id) => {
            let track = analysis
                .tracks
                .iter()
                .find(|track| track.id == id)
                .ok_or("unknown feed ID")?;
            json!({
                "feed_id": track.id,
                "appearances": analysis.feed_appearances(id, analysis.start, analysis.end).collect::<Vec<_>>(),
                "changes": analysis.events.iter().filter(|event| event.feed_id() == id).collect::<Vec<_>>(),
            })
        }
    };
    write_json(BufWriter::new(io::stdout().lock()), &report)
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    let command = match args.next() {
        None => {
            println!("{HELP}");
            return Ok(());
        }
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("{HELP}");
            return Ok(());
        }
        Some(arg) => arg,
    };
    if matches!(args.peek().map(String::as_str), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    let file = PathBuf::from(next_value(&mut args, &command)?);
    match command.as_str() {
        "scan" => scan(file, args),
        "inspect" => inspect(file, args),
        _ => Err(format!("unknown command {command}").into()),
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tiletrail: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
