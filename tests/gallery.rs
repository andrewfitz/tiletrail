use std::collections::HashMap;
use tiletrail::*;

const CELL_WIDTH: u32 = 64;
const CELL_HEIGHT: u32 = 40;
const GUTTER: u32 = 6;

fn dimensions(columns: u32, rows: u32) -> (u32, u32) {
    (columns * CELL_WIDTH + GUTTER, rows * CELL_HEIGHT + GUTTER)
}

fn gallery(columns: u32, rows: u32, sources: &[u32], matte: u8) -> Vec<u8> {
    let (width, height) = dimensions(columns, rows);
    let mut image = vec![matte; (width * height * 3) as usize];
    for (cell, &source) in sources.iter().enumerate() {
        let x0 = (cell as u32 % columns) * CELL_WIDTH + GUTTER;
        let y0 = (cell as u32 / columns) * CELL_HEIGHT + GUTTER;
        for y in 0..CELL_HEIGHT - GUTTER {
            for x in 0..CELL_WIDTH - GUTTER {
                // Use source-local patterns so an identity stays recognizable
                // when the layout moves it to another cell.
                let block = y * 4 / (CELL_HEIGHT - GUTTER) * 4 + x * 4 / (CELL_WIDTH - GUTTER);
                let seed = (source + 3)
                    .wrapping_mul(0x9e3779b9)
                    .rotate_left(block * 7 % 32)
                    ^ block.wrapping_mul(0x85ebca6b);
                let offset = ((y0 + y) * width + x0 + x) as usize * 3;
                for channel in 0..3 {
                    image[offset + channel] = 52 + (seed >> (8 * channel) & 0x9f) as u8;
                }
            }
        }
    }
    image
}

fn analyze(columns: u32, rows: u32, frames: &[Vec<u32>], sampling: Sampling) -> AnalysisResult {
    let time = |index: i64| Timestamp::new(index, 1, 2);
    let mut analyzer = Analyzer::with_layout(
        AnalyzerConfig {
            mode: BackendMode::CpuOnly,
            sampling,
        },
        time(0),
        FeedLayout::Gallery { columns, rows },
    )
    .unwrap();
    let (width, height) = dimensions(columns, rows);
    for (index, sources) in frames.iter().enumerate() {
        analyzer
            .push(Frame {
                data: &gallery(columns, rows, sources, 18),
                width,
                height,
                stride: width as usize * 3,
                format: PixelFormat::Rgb24,
                timestamp: time(index as i64),
                display: DisplayGeometry::default(),
            })
            .unwrap();
    }
    analyzer.finish(time(frames.len() as i64)).unwrap()
}

fn score(columns: u32, rows: u32, frames: &[Vec<u32>], analysis: &AnalysisResult) {
    let (width, height) = dimensions(columns, rows);
    let mut source_to_feed = HashMap::new();
    let mut feed_to_source = HashMap::new();
    let mut missed = 0;
    let mut false_tiles = 0;
    let mut swapped = 0;
    for (observation, truth) in analysis.observations.iter().zip(frames) {
        let mut matched = vec![false; truth.len()];
        for tile in &observation.regions {
            let x = tile.rect.x * width as f32;
            let y = tile.rect.y * height as f32;
            let w = tile.rect.width * width as f32;
            let h = tile.rect.height * height as f32;
            let col = (x / CELL_WIDTH as f32).floor() as u32;
            let row = (y / CELL_HEIGHT as f32).floor() as u32;
            let index = (row * columns + col) as usize;
            if col >= columns
                || row >= rows
                || index >= truth.len()
                || matched[index]
                || (x - (col * CELL_WIDTH + GUTTER) as f32).abs() > 3.0
                || (y - (row * CELL_HEIGHT + GUTTER) as f32).abs() > 3.0
                || (w - (CELL_WIDTH - GUTTER) as f32).abs() > 4.0
                || (h - (CELL_HEIGHT - GUTTER) as f32).abs() > 4.0
            {
                false_tiles += 1;
                continue;
            }
            matched[index] = true;
            let source = truth[index];
            let Some(feed) = tile.feed_id() else {
                swapped += 1;
                continue;
            };
            if *source_to_feed.entry(source).or_insert(feed) != feed
                || *feed_to_source.entry(feed).or_insert(source) != source
            {
                swapped += 1;
            }
        }
        missed += matched.iter().filter(|&&value| !value).count();
    }
    assert_eq!(analysis.observations.len(), frames.len());
    assert_eq!((missed, false_tiles, swapped), (0, 0, 0));
}

#[test]
fn tracks_40_content_identities_through_a_25_tile_gallery() {
    let first: Vec<_> = (0..25).collect();
    let moved: Vec<_> = (0..25).map(|n| (n + 7) % 25).collect();
    let changed: Vec<_> = (15..40).collect();
    let frames = [first.clone(), moved, changed, first];
    for sampling in [Sampling::EveryFrame, Sampling::Sparse] {
        let analysis = analyze(5, 5, &frames, sampling);
        score(5, 5, &frames, &analysis);
        assert_eq!(analysis.tracks.len(), 40);
    }
}

#[test]
fn recovers_the_full_gallery_at_the_64_box_limit() {
    let frames = [(0..64).collect()];
    let analysis = analyze(8, 8, &frames, Sampling::EveryFrame);
    score(8, 8, &frames, &analysis);
    assert_eq!(analysis.tracks.len(), 64);
}

#[test]
fn hint_does_not_force_a_grid_on_a_single_surface() {
    let analysis = analyze(5, 5, &[vec![]], Sampling::EveryFrame);
    assert!(analysis.observations[0].regions.len() < 25);
}

#[test]
fn hints_use_display_dimensions_and_recognize_bright_gutters() {
    let (width, height) = dimensions(3, 2);
    let time = |index| Timestamp::new(index, 1, 30);
    let mut analyzer = Analyzer::with_layout(
        AnalyzerConfig {
            mode: BackendMode::CpuOnly,
            sampling: Sampling::EveryFrame,
        },
        time(0),
        FeedLayout::Gallery {
            columns: 2,
            rows: 3,
        },
    )
    .unwrap();
    analyzer
        .push(Frame {
            data: &gallery(3, 2, &[0, 1, 2, 3, 4, 5], 252),
            width,
            height,
            stride: width as usize * 3,
            format: PixelFormat::Rgb24,
            timestamp: time(0),
            display: DisplayGeometry {
                rotation: Rotation::R90,
                ..DisplayGeometry::default()
            },
        })
        .unwrap();
    let result = analyzer.finish(time(1)).unwrap();
    assert_eq!(result.observations[0].regions.len(), 6);
    for region in &result.observations[0].regions {
        assert!((region.rect.width - (CELL_HEIGHT - GUTTER) as f32 / height as f32).abs() < 0.03);
        assert!((region.rect.height - (CELL_WIDTH - GUTTER) as f32 / width as f32).abs() < 0.03);
        assert_eq!(region.confidence, 0.9);
    }
}
