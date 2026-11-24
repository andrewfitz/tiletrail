use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use tiletrail::{DisplayGeometry, Rotation, TimeBase, Timestamp};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Stamp {
    bytes: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    #[serde(skip)]
    inode: u64,
    #[serde(skip)]
    device: u64,
    #[serde(skip)]
    change_seconds: i64,
    #[serde(skip)]
    change_nanoseconds: i64,
}

#[cfg(unix)]
pub fn stamp(path: &Path) -> Result<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err("input must be a regular file".into());
    }
    Ok(Stamp {
        bytes: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        inode: metadata.ino(),
        device: metadata.dev(),
        change_seconds: metadata.ctime(),
        change_nanoseconds: metadata.ctime_nsec(),
    })
}

#[cfg(not(unix))]
pub fn stamp(path: &Path) -> Result<Stamp> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err("input must be a regular file".into());
    }
    let time = metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?;
    Ok(Stamp {
        bytes: metadata.len(),
        modified_seconds: time.as_secs().try_into()?,
        modified_nanoseconds: time.subsec_nanos().into(),
        inode: 0,
        device: 0,
        change_seconds: 0,
        change_nanoseconds: 0,
    })
}

pub fn digest(path: &Path) -> Result<String> {
    let mut input = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    io::copy(&mut input, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Deserialize)]
struct Stream {
    index: u32,
    width: u32,
    height: u32,
    time_base: String,
    start_pts: Option<i64>,
    duration_ts: Option<i64>,
    sample_aspect_ratio: Option<String>,
    disposition: Disposition,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct Disposition {
    attached_pic: u32,
}

#[derive(Deserialize)]
struct SideData {
    #[serde(default)]
    side_data_type: String,
    displaymatrix: Option<String>,
    rotation: Option<i32>,
}

#[derive(Deserialize)]
struct Metadata {
    streams: Vec<Stream>,
    format: Format,
}

#[derive(Deserialize)]
struct Format {
    format_name: String,
}

#[derive(Deserialize)]
struct Frames {
    frames: Vec<VideoFrame>,
}

#[derive(Deserialize)]
struct VideoFrame {
    best_effort_timestamp: i64,
    duration: Option<i64>,
    pkt_duration: Option<i64>,
    width: u32,
    height: u32,
    sample_aspect_ratio: Option<String>,
}

pub struct Media {
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub time_base: TimeBase,
    pub origin: i64,
    pub duration: i64,
    pub display: DisplayGeometry,
    frames: Vec<VideoFrame>,
}

fn probe<T: for<'a> Deserialize<'a>>(file: &Path, args: &[&str]) -> Result<T> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-protocol_whitelist", "file", "-of", "json"])
        .args(args)
        .arg(file)
        .output()?;
    if !output.status.success() {
        return Err(format!("ffprobe: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn ratio(value: &str, delimiter: char) -> Result<(u32, u32)> {
    let (a, b) = value.split_once(delimiter).ok_or("invalid media ratio")?;
    let (a, b) = (a.parse()?, b.parse()?);
    if a == 0 || b == 0 {
        return Err("invalid media ratio".into());
    }
    Ok((a, b))
}

fn pixel_aspect(value: Option<&str>) -> Result<(u32, u32)> {
    match value {
        None | Some("N/A" | "0:1") => Ok((1, 1)),
        Some(value) => ratio(value, ':'),
    }
}

fn rotation(side_data: &[SideData]) -> Result<Rotation> {
    let Some(matrix) = side_data
        .iter()
        .find(|item| item.side_data_type == "Display Matrix")
    else {
        return Ok(Rotation::R0);
    };
    let turn = matrix
        .rotation
        .ok_or("missing display rotation")?
        .rem_euclid(360);
    let (rotation, a, b) = match turn {
        0 => (Rotation::R0, 65536, 0),
        270 => (Rotation::R90, 0, 65536),
        180 => (Rotation::R180, -65536, 0),
        90 => (Rotation::R270, 0, -65536),
        _ => return Err("unsupported display rotation".into()),
    };
    let mut values = Vec::new();
    for line in matrix
        .displaymatrix
        .as_deref()
        .ok_or("missing display matrix")?
        .lines()
    {
        if let Some((_, row)) = line.split_once(':') {
            for value in row.split_whitespace() {
                values.push(value.parse::<i32>()?);
            }
        }
    }
    if values != [a, b, 0, -b, a, 0, 0, 0, 1073741824] {
        return Err("unsupported display matrix".into());
    }
    Ok(rotation)
}

impl Media {
    pub fn open(path: &Path, requested_stream: Option<u32>) -> Result<Self> {
        let metadata: Metadata = probe(
            path,
            &[
                "-select_streams",
                "v",
                "-show_entries",
                "stream=index,width,height,time_base,start_pts,duration_ts,sample_aspect_ratio:stream_disposition=attached_pic:stream_tags=duration,DURATION:stream_side_data=side_data_type,rotation,displaymatrix:format=format_name",
            ],
        )?;
        if metadata
            .format
            .format_name
            .split(',')
            .any(|name| matches!(name, "concat" | "hls" | "dash" | "image2" | "image2pipe"))
        {
            return Err("scan requires a recording stored in a single media file".into());
        }
        let stream = metadata
            .streams
            .into_iter()
            .find(|stream| {
                stream.disposition.attached_pic == 0
                    && requested_stream.is_none_or(|index| stream.index == index)
            })
            .ok_or("input has no matching video stream")?;
        let (numerator, denominator) = ratio(&stream.time_base, '/')?;
        let (pixel_aspect_num, pixel_aspect_den) =
            pixel_aspect(stream.sample_aspect_ratio.as_deref())?;
        let frame_info: Frames = probe(
            path,
            &[
                "-select_streams",
                &stream.index.to_string(),
                "-show_frames",
                "-show_entries",
                "frame=best_effort_timestamp,duration,pkt_duration,width,height,sample_aspect_ratio",
            ],
        )?;
        let frames = frame_info.frames;
        let first = frames.first().ok_or("input has no decoded video frames")?;
        let origin = stream.start_pts.unwrap_or(first.best_effort_timestamp);
        let last_pts = frames.last().unwrap().best_effort_timestamp;
        let format = |name| {
            metadata
                .format
                .format_name
                .split(',')
                .any(|value| value == name)
        };
        let mut duration = if format("mov")
            && let Some(duration) = stream.duration_ts
            && duration > last_pts.checked_sub(origin).ok_or("timestamp overflow")?
        {
            duration
        } else {
            let mut packet_end = last_pts;
            for (index, frame) in frames.iter().enumerate() {
                let pts = frame.best_effort_timestamp;
                let ticks = frame
                    .duration
                    .or(frame.pkt_duration)
                    .filter(|&ticks| ticks > 0)
                    .or_else(|| {
                        frames
                            .get(index + 1)?
                            .best_effort_timestamp
                            .checked_sub(pts)
                    })
                    .filter(|&ticks| ticks > 0)
                    .ok_or("duration of a video frame is unknown")?;
                packet_end = packet_end.max(pts.checked_add(ticks).ok_or("timestamp overflow")?);
            }
            packet_end.checked_sub(origin).ok_or("timestamp overflow")?
        };
        if format("matroska")
            && let Some((_, tag)) = stream
                .tags
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("duration"))
            && let Ok(tag) = super::timestamp(tag)
            && tag.pts >= 0
        {
            // Matroska block durations round down to whole ticks. Use the track
            // tag only if it corroborates the end to within one source tick.
            let top = i128::from(tag.pts) * i128::from(denominator);
            let bottom = i128::from(tag.time_base.denominator) * i128::from(numerator);
            if let Ok(tagged_end) = i64::try_from((top + bottom / 2) / bottom)
                && tagged_end > last_pts
                && let Some(tagged_duration) = tagged_end.checked_sub(origin)
                && tagged_duration.abs_diff(duration) <= 1
            {
                duration = tagged_duration;
            }
        }
        if duration <= 0 {
            return Err("video has no usable presentation range".into());
        }
        for frame in &frames {
            if frame.width != stream.width
                || frame.height != stream.height
                || frame.sample_aspect_ratio.is_some()
                    && pixel_aspect(frame.sample_aspect_ratio.as_deref())?
                        != (pixel_aspect_num, pixel_aspect_den)
            {
                return Err("video geometry changes during the recording".into());
            }
        }
        Ok(Self {
            index: stream.index,
            width: stream.width,
            height: stream.height,
            time_base: TimeBase {
                numerator,
                denominator,
            },
            origin,
            duration,
            display: DisplayGeometry {
                rotation: rotation(&stream.side_data_list)?,
                pixel_aspect_num,
                pixel_aspect_den,
            },
            frames,
        })
    }

    pub fn decode(
        &self,
        path: &Path,
        mut visit: impl FnMut(Timestamp, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let size = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or("video dimensions overflow")?;
        if size == 0 {
            return Err("invalid video dimensions".into());
        }
        let mut decoder = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-xerror",
                "-nostdin",
                "-copyts",
                "-noautorotate",
                "-protocol_whitelist",
                "file",
                "-i",
            ])
            .arg(path)
            .args([
                "-map",
                &format!("0:{}", self.index),
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "rgb24",
                "-threads:v",
                "1",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .spawn()?;
        let result = (|| -> Result<()> {
            let mut stream = decoder.stdout.take().ok_or("missing decoder output")?;
            let mut pixels = vec![0; size];
            for frame in &self.frames {
                stream.read_exact(&mut pixels)?;
                let pts = frame
                    .best_effort_timestamp
                    .checked_sub(self.origin)
                    .ok_or("timestamp overflow")?;
                if (0..self.duration).contains(&pts) {
                    visit(
                        Timestamp {
                            pts,
                            time_base: self.time_base,
                        },
                        &pixels,
                    )?;
                }
            }
            if stream.read(&mut [0])? != 0 {
                return Err("decoder returned more frames than ffprobe".into());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = decoder.kill();
        }
        let status = decoder.wait()?;
        result?;
        if !status.success() {
            return Err(format!("ffmpeg exited with {status}").into());
        }
        Ok(())
    }
}
