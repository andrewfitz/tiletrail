use crate::{Frame, PixelFormat, Rect, Rotation, TileKind, TileRole};

const SAMPLE_LIMIT: u32 = 256;
const MAX_TILES: usize = 64;
const GRID_SIZE: usize = 4;
const GRID_VALUES: usize = GRID_SIZE * GRID_SIZE * 3;
const HISTOGRAM_BINS: usize = 8;
const DESCRIPTOR_VALUES: usize = GRID_VALUES + 3 * HISTOGRAM_BINS;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Measurements {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
    pub rotation: Rotation,
}

pub(crate) fn sample_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let longest = u64::from(width.max(height));
    if longest <= u64::from(SAMPLE_LIMIT) {
        return (width, height);
    }
    let scale =
        |size| ((u64::from(size) * u64::from(SAMPLE_LIMIT) + longest / 2) / longest).max(1) as u32;
    (scale(width), scale(height))
}

pub(crate) fn measure(frame: &Frame<'_>) -> Measurements {
    let (width, height) = sample_dimensions(frame.width, frame.height);
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    let bytes_per_pixel = frame.format.bytes_per_pixel();
    let sample_at = |i, source, target| {
        (((2 * u64::from(i) + 1) * u64::from(source)) / (2 * u64::from(target))) as usize
    };
    for y in 0..height {
        let row = sample_at(y, frame.height, height) * frame.stride;
        for x in 0..width {
            let offset = row + sample_at(x, frame.width, width) * bytes_per_pixel;
            let pixel = &frame.data[offset..];
            pixels.push(match frame.format {
                PixelFormat::Gray8 => [pixel[0]; 3],
                PixelFormat::Rgb24 | PixelFormat::Rgba8 => [pixel[0], pixel[1], pixel[2]],
                PixelFormat::Bgra8 => [pixel[2], pixel[1], pixel[0]],
            });
        }
    }
    Measurements {
        width,
        height,
        pixels,
        rotation: frame.display.rotation,
    }
}

#[derive(Clone)]
pub(crate) struct Tile {
    pub rect: Rect,
    pub kind: TileKind,
    pub role: TileRole,
    pub descriptor: Vec<f32>,
    pub confidence: f32,
}

pub(crate) struct Detection {
    pub tiles: Vec<Tile>,
    pub confidence: f32,
}

#[derive(Clone, Copy)]
struct Bounds {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl Bounds {
    fn area(self) -> usize {
        (self.right - self.left) * (self.bottom - self.top)
    }

    fn dimensions(self, horizontal: bool) -> (usize, usize, usize, usize) {
        if horizontal {
            (self.top, self.bottom, self.left, self.right)
        } else {
            (self.left, self.right, self.top, self.bottom)
        }
    }

    fn with_extent(mut self, horizontal: bool, start: usize, end: usize) -> Self {
        if horizontal {
            self.top = start;
            self.bottom = end;
        } else {
            self.left = start;
            self.right = end;
        }
        self
    }

    fn display_rect(self, image: &Measurements) -> Rect {
        let x = self.left as f32 / image.width as f32;
        let y = self.top as f32 / image.height as f32;
        let width = (self.right - self.left) as f32 / image.width as f32;
        let height = (self.bottom - self.top) as f32 / image.height as f32;
        let reverse_x = (image.width as usize - self.right) as f32 / image.width as f32;
        let reverse_y = (image.height as usize - self.bottom) as f32 / image.height as f32;
        let (x, y, width, height) = match image.rotation {
            Rotation::R0 => (x, y, width, height),
            Rotation::R90 => (reverse_y, x, height, width),
            Rotation::R180 => (reverse_x, reverse_y, width, height),
            Rotation::R270 => (y, reverse_x, height, width),
        };
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl Measurements {
    fn at(&self, x: usize, y: usize) -> [u8; 3] {
        self.pixels[y * self.width as usize + x]
    }

    fn at_line(&self, horizontal: bool, line: usize, i: usize) -> [u8; 3] {
        if horizontal {
            self.at(i, line)
        } else {
            self.at(line, i)
        }
    }
}

fn color_gap(a: [u8; 3], b: [u8; 3]) -> u8 {
    a[0].abs_diff(b[0])
        .max(a[1].abs_diff(b[1]))
        .max(a[2].abs_diff(b[2]))
}

// Use a neutral dark frame-edge color as possible matte. Perimeter sampling leaves
// the backgrounds of colorful edge-to-edge feeds out of the matte model.
fn matte_color(image: &Measurements, bounds: Bounds) -> Option<[u8; 3]> {
    let mut buckets = [0_usize; 8];
    for horizontal in [false, true] {
        let (start, end, cross_start, cross_end) = bounds.dimensions(horizontal);
        for line in [start, end - 1] {
            for i in cross_start..cross_end {
                let pixel = image.at_line(horizontal, line, i);
                let high = *pixel.iter().max().unwrap();
                let low = *pixel.iter().min().unwrap();
                if high < 64 && high - low <= 8 {
                    let level =
                        (u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2])) / 3;
                    buckets[level as usize / 8] += 1;
                }
            }
        }
    }
    let (index, support) = buckets
        .into_iter()
        .enumerate()
        .max_by_key(|(_, count)| *count)?;
    let perimeter = 2 * (bounds.right + bounds.bottom);
    if support * 10 >= perimeter {
        return Some([(index * 8 + 3) as u8; 3]);
    }

    // A narrow black gutter can touch two opposite edges without making up much
    // of the perimeter. Require a full crossing before accepting black as matte.
    let black = [3; 3];
    for horizontal in [true, false] {
        let (start, end, cross_start, cross_end) = bounds.dimensions(horizontal);
        if end - start < 3 {
            continue;
        }
        let crosses_frame = (start + 1..end - 1).any(|line| {
            color_gap(image.at_line(horizontal, line, cross_start), black) <= 9
                && color_gap(image.at_line(horizontal, line, cross_end - 1), black) <= 9
                && matte_line(image, black, horizontal, line, cross_start, cross_end)
        });
        if crosses_frame {
            return Some(black);
        }
    }
    None
}

fn matte_line(
    image: &Measurements,
    matte: [u8; 3],
    horizontal: bool,
    line: usize,
    cross_start: usize,
    cross_end: usize,
) -> bool {
    let count = (cross_start..cross_end)
        .filter(|&i| color_gap(image.at_line(horizontal, line, i), matte) <= 9)
        .count();
    count * 100 >= (cross_end - cross_start) * 96
}

fn trim_matte(image: &Measurements, mut bounds: Bounds, matte: [u8; 3]) -> Bounds {
    // A second pass picks up side bars once wide top or bottom bars are eliminated.
    for horizontal in [true, false, true, false] {
        let (mut start, mut end, cross_start, cross_end) = bounds.dimensions(horizontal);
        while start < end && matte_line(image, matte, horizontal, start, cross_start, cross_end) {
            start += 1;
        }
        while start < end && matte_line(image, matte, horizontal, end - 1, cross_start, cross_end) {
            end -= 1;
        }
        bounds = bounds.with_extent(horizontal, start, end);
        if bounds.area() == 0 {
            break;
        }
    }
    bounds
}

#[derive(Clone, Copy)]
struct Split {
    first: Bounds,
    second: Bounds,
    score: f32,
}

fn best_split(image: &Measurements, bounds: Bounds, matte: Option<[u8; 3]>) -> Option<Split> {
    let mut best: Option<Split> = None;
    for horizontal in [true, false] {
        let (start, end, cross_start, cross_end) = bounds.dimensions(horizontal);
        let min_extent = (if horizontal {
            image.height
        } else {
            image.width
        } / 20)
            .clamp(4, 12) as usize;
        if end - start < 2 * min_extent || cross_end - cross_start < 4 {
            continue;
        }

        let mut line = start + min_extent;
        while line <= end - min_extent {
            let mut gutter_end = line;
            if let Some(matte) = matte {
                while gutter_end < end - min_extent
                    && matte_line(image, matte, horizontal, gutter_end, cross_start, cross_end)
                {
                    gutter_end += 1;
                }
            }
            let score = if gutter_end > line {
                // Distinguish a gutter from a flat dark band inside a dark feed.
                let lit = |line| {
                    (cross_start..cross_end)
                        .filter(|&i| {
                            color_gap(image.at_line(horizontal, line, i), matte.unwrap()) > 14
                        })
                        .count()
                };
                let occupied =
                    lit(line - 1).min(lit(gutter_end)) as f32 / (cross_end - cross_start) as f32;
                // Centered rows can leave most of a seam adjacent to the matte.
                if occupied >= 0.30 {
                    0.65 + 0.35 * occupied
                } else {
                    0.0
                }
            } else {
                let mut support = 0;
                let mut salient = 0;
                for i in cross_start..cross_end {
                    let a = image.at_line(horizontal, line - 1, i);
                    let b = image.at_line(horizontal, line, i);
                    let gap = color_gap(a, b);
                    support += usize::from(gap >= 22);
                    let nearby = color_gap(a, image.at_line(horizontal, line - 2, i))
                        .max(color_gap(b, image.at_line(horizontal, line + 1, i)));
                    salient += usize::from(gap >= 26 && gap.saturating_sub(nearby) >= 12);
                }
                let length = (cross_end - cross_start) as f32;
                if support as f32 / length < 0.76 {
                    0.0
                } else {
                    salient as f32 / length
                }
            };
            if score >= 0.70 {
                let balance = (line - start).min(end - gutter_end) as f32 / (end - start) as f32;
                let candidate = Split {
                    first: bounds.with_extent(horizontal, start, line),
                    second: bounds.with_extent(horizontal, gutter_end, end),
                    score: score * 0.87
                        + balance * 0.10
                        + if gutter_end > line { 0.10 } else { 0.0 },
                };
                if best.is_none_or(|existing| candidate.score > existing.score) {
                    best = Some(candidate);
                }
            }
            line = (line + 1).max(gutter_end);
        }
    }
    best
}

fn describe(image: &Measurements, bounds: Bounds) -> (Vec<f32>, TileKind, bool) {
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let mut descriptor = Vec::with_capacity(DESCRIPTOR_VALUES);
    let mut histogram = [[0_u32; HISTOGRAM_BINS]; 3];
    let mut neutral = 0;
    let mut bright_neutral = 0;
    for grid_y in 0..GRID_SIZE {
        let top = bounds.top + grid_y * height / GRID_SIZE;
        let bottom = (bounds.top + (grid_y + 1) * height / GRID_SIZE).max(top + 1);
        for grid_x in 0..GRID_SIZE {
            let left = bounds.left + grid_x * width / GRID_SIZE;
            let right = (bounds.left + (grid_x + 1) * width / GRID_SIZE).max(left + 1);
            let mut sum = [0_u32; 3];
            for y in top..bottom {
                for x in left..right {
                    let pixel = image.at(x, y);
                    for channel in 0..3 {
                        sum[channel] += u32::from(pixel[channel]);
                        histogram[channel][usize::from(pixel[channel]) / 32] += 1;
                    }
                    let max = *pixel.iter().max().unwrap();
                    let min = *pixel.iter().min().unwrap();
                    neutral += usize::from(max - min <= 12);
                    bright_neutral += usize::from(min >= 180 && max - min <= 20);
                }
            }
            descriptor.extend(
                sum.map(|value| value as f32 / (255 * (bottom - top) * (right - left)) as f32),
            );
        }
    }
    let histogram_size: u32 = histogram[0].iter().sum();
    let flat = histogram.iter().all(|bins| {
        bins.windows(2)
            .any(|pair| (pair[0] + pair[1]) * 100 >= histogram_size * 88)
    });
    let kind = if flat
        && neutral as f32 / histogram_size as f32 >= 0.85
        && bright_neutral as f32 / histogram_size as f32 <= 0.5
    {
        TileKind::CameraOff
    } else {
        TileKind::Video
    };
    let presentation_hint = bright_neutral as f32 / histogram_size as f32 >= 0.50;
    descriptor.extend(
        histogram
            .into_iter()
            .flatten()
            .map(|bin| bin as f32 / histogram_size as f32),
    );
    (descriptor, kind, presentation_hint)
}

pub(crate) fn appearance_distance(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != DESCRIPTOR_VALUES || right.len() != DESCRIPTOR_VALUES {
        return 1.0;
    }
    let differences = left.iter().zip(right).map(|(a, b)| (a - b).abs());
    let squared_gap = differences
        .clone()
        .take(GRID_VALUES)
        .map(|d| d * d)
        .sum::<f32>();
    let grid_gap = (squared_gap / GRID_VALUES as f32).sqrt();
    let hist_gap = differences.skip(GRID_VALUES).sum::<f32>() / 6.0;
    (0.85 * grid_gap + 0.15 * hist_gap).clamp(0.0, 1.0)
}

pub(crate) fn detect(image: &Measurements) -> Detection {
    if image.width == 0 || image.height == 0 {
        return Detection {
            tiles: Vec::new(),
            confidence: 0.0,
        };
    }
    let whole_frame = Bounds {
        left: 0,
        top: 0,
        right: image.width as usize,
        bottom: image.height as usize,
    };
    let matte = matte_color(image, whole_frame);
    let mut pending = vec![(whole_frame, 1.0_f32)];
    let mut regions = Vec::new();
    while let Some((bounds, confidence)) = pending.pop() {
        let trimmed = matte.map_or(bounds, |color| trim_matte(image, bounds, color));
        if trimmed.area() == 0 {
            continue;
        }
        if pending.len() + regions.len() + 1 < MAX_TILES {
            if let Some(split) = best_split(image, trimmed, matte) {
                let confidence = confidence.min(split.score);
                pending.push((split.second, confidence));
                pending.push((split.first, confidence));
                continue;
            }
        }
        regions.push((trimmed, confidence));
    }

    let largest = regions
        .iter()
        .map(|(bounds, _)| bounds.area())
        .max()
        .unwrap_or(0);
    let stage_layout = regions.len() > 1
        && regions
            .iter()
            .any(|(bounds, _)| bounds.area() * 3 <= largest);
    let solo = regions.len() == 1;
    let mut tiles: Vec<_> = regions
        .into_iter()
        .map(|(bounds, confidence)| {
            let (descriptor, mut kind, presentation_hint) = describe(image, bounds);
            let role = if stage_layout && bounds.area() * 3 <= largest {
                TileRole::Thumbnail
            } else {
                if stage_layout && bounds.area() == largest && presentation_hint {
                    kind = TileKind::ScreenShare;
                }
                TileRole::Primary
            };
            // An unpartitioned image provides weak evidence that the source is a call.
            let confidence = if solo {
                0.55
            } else {
                confidence.clamp(0.0, 1.0)
            };
            Tile {
                rect: bounds.display_rect(image),
                kind,
                role,
                descriptor,
                confidence,
            }
        })
        .collect();
    tiles.sort_by(|a, b| {
        a.rect
            .y
            .total_cmp(&b.rect.y)
            .then(a.rect.x.total_cmp(&b.rect.x))
    });
    let confidence = tiles
        .iter()
        .map(|tile| tile.confidence)
        .reduce(f32::min)
        .unwrap_or(0.0);
    Detection { tiles, confidence }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DisplayGeometry, Timestamp};

    const RED: [u8; 3] = [190, 49, 33];
    const GREEN: [u8; 3] = [45, 163, 64];
    const BLUE: [u8; 3] = [36, 63, 176];
    const GRAY: [u8; 3] = [82; 3];

    fn scene(paint: &[(usize, usize, usize, usize, [u8; 3])]) -> Measurements {
        let mut image = Measurements {
            width: 160,
            height: 96,
            pixels: vec![[7; 3]; 160 * 96],
            rotation: Rotation::R0,
        };
        for &(left, top, width, height, color) in paint {
            for y in top..top + height {
                image.pixels[y * 160 + left..y * 160 + left + width].fill(color);
            }
        }
        image
    }

    fn assert_box(tile: &Tile, normalized: [f32; 4]) {
        for (actual, expected) in [tile.rect.x, tile.rect.y, tile.rect.width, tile.rect.height]
            .into_iter()
            .zip(normalized)
        {
            assert!(
                (actual - expected).abs() < 0.0001,
                "{:?} versus {normalized:?}",
                tile.rect
            );
        }
    }

    #[test]
    fn padded_bgra_measurement_ignores_alpha_and_preserves_rotation() {
        let data = [
            30, 20, 10, 1, 60, 50, 40, 2, 255, 255, 90, 80, 70, 4, 120, 110, 100, 5,
        ];
        let frame = Frame {
            data: &data,
            width: 2,
            height: 2,
            stride: 10,
            format: PixelFormat::Bgra8,
            timestamp: Timestamp::new(0, 1, 30),
            display: DisplayGeometry {
                rotation: Rotation::R90,
                ..Default::default()
            },
        };
        let measured = measure(&frame);
        assert_eq!(
            measured.pixels,
            [[10, 20, 30], [40, 50, 60], [70, 80, 90], [100, 110, 120]]
        );
        assert_eq!(measured.rotation, Rotation::R90);
        assert_eq!(sample_dimensions(1920, 1080), (256, 144));
        assert_eq!(sample_dimensions(1, u32::MAX), (1, 256));
    }

    #[test]
    fn downsampling_reads_the_center_pixel_of_each_sampling_interval() {
        let data: Vec<_> = (0..512).map(|x| if x % 2 == 0 { 5 } else { 25 }).collect();
        let frame = Frame {
            data: &data,
            width: 512,
            height: 1,
            stride: data.len(),
            format: PixelFormat::Gray8,
            timestamp: Timestamp::new(0, 1, 30),
            display: Default::default(),
        };
        let measured = measure(&frame);
        assert_eq!((measured.width, measured.height), (256, 1));
        assert_eq!(measured.pixels, vec![[25; 3]; 256]);
    }

    #[test]
    fn centered_single_guest_does_not_split_at_upper_row_seams() {
        let image = scene(&[
            (1, 1, 50, 45, RED),
            (55, 1, 50, 45, GREEN),
            (109, 1, 50, 45, BLUE),
            (52, 50, 56, 45, GRAY),
        ]);
        let result = detect(&image);
        assert_eq!(result.tiles.len(), 4);
        assert_box(
            &result.tiles[3],
            [52.0 / 160.0, 50.0 / 96.0, 56.0 / 160.0, 45.0 / 96.0],
        );
        assert_eq!(result.tiles[3].kind, TileKind::CameraOff);
        assert_eq!(result.tiles[3].role, TileRole::Primary);
    }

    #[test]
    fn flush_gallery_boundaries_and_clockwise_display_coordinates() {
        let mut image = scene(&[
            (0, 0, 80, 48, RED),
            (80, 0, 80, 48, GREEN),
            (0, 48, 80, 48, BLUE),
            (80, 48, 80, 48, GRAY),
        ]);
        image.rotation = Rotation::R90;
        let result = detect(&image);
        assert_eq!(result.tiles.len(), 4);
        assert_box(&result.tiles[0], [0.0, 0.0, 0.5, 0.5]);
        assert_eq!(result.tiles[2].kind, TileKind::CameraOff);
    }

    #[test]
    fn dominant_presentation_with_video_strip() {
        let image = scene(&[
            (0, 0, 120, 96, [225; 3]),
            (124, 8, 36, 38, RED),
            (124, 50, 36, 38, BLUE),
        ]);
        let result = detect(&image);
        assert_eq!(result.tiles.len(), 3);
        assert_eq!(result.tiles[0].kind, TileKind::ScreenShare);
        assert_eq!(result.tiles[0].role, TileRole::Primary);
        assert_eq!(result.tiles[1].role, TileRole::Thumbnail);
        assert_eq!(result.tiles[2].role, TileRole::Thumbnail);
    }

    #[test]
    fn descriptor_compares_same_view_across_sizes_and_different_feeds() {
        let first = detect(&scene(&[(1, 1, 70, 45, RED)]));
        let second = detect(&scene(&[(80, 5, 60, 70, RED)]));
        let other = detect(&scene(&[(80, 5, 60, 70, BLUE)]));
        let descriptor = |detection: &Detection| detection.tiles[0].descriptor.clone();
        assert_eq!(
            appearance_distance(&descriptor(&first), &descriptor(&second)),
            0.0
        );
        assert!(appearance_distance(&descriptor(&first), &descriptor(&other)) > 0.4);
    }

    #[test]
    fn black_gutter_touching_only_two_frame_edges_is_not_a_tile() {
        let result = detect(&scene(&[(0, 0, 76, 96, RED), (84, 0, 76, 96, BLUE)]));
        assert_eq!(result.tiles.len(), 2);
        assert_box(&result.tiles[0], [0.0, 0.0, 76.0 / 160.0, 1.0]);
        assert_box(&result.tiles[1], [84.0 / 160.0, 0.0, 76.0 / 160.0, 1.0]);
    }

    #[test]
    fn active_speaker_border_preserves_tile_bounds_and_appearance() {
        let before = detect(&scene(&[(0, 0, 78, 96, RED), (82, 0, 78, 96, BLUE)]));
        let after = detect(&scene(&[
            (0, 0, 78, 96, [230, 198, 28]),
            (2, 2, 74, 92, RED),
            (82, 0, 78, 96, BLUE),
        ]));
        assert_eq!(before.tiles.len(), 2);
        assert_eq!(after.tiles.len(), 2);
        assert_eq!(before.tiles[0].rect, after.tiles[0].rect);
        assert!(
            appearance_distance(&before.tiles[0].descriptor, &after.tiles[0].descriptor) < 0.10
        );
    }

    #[test]
    fn black_frame_has_no_detected_tiles() {
        assert!(detect(&scene(&[])).tiles.is_empty());
    }
}
