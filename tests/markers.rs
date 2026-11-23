use tiletrail::*;

fn time(pts: i64) -> Timestamp {
    Timestamp::new(pts, 1, 1000)
}

fn frame(pts: i64, occupants: &[u64]) -> FrameObservation {
    let width = 1.0 / occupants.len() as f32;
    FrameObservation {
        timestamp: time(pts),
        display: DisplayGeometry::default(),
        confidence: 1.0,
        regions: occupants
            .iter()
            .enumerate()
            .map(|(slot, &id)| RegionObservation {
                appearance_id: id,
                participant_id: Some(id),
                rect: Rect {
                    x: slot as f32 * width,
                    y: 0.0,
                    width,
                    height: 1.0,
                },
                kind: TileKind::Video,
                role: TileRole::Primary,
                confidence: 1.0,
                identity_confidence: Some(1.0),
                identity_uncertain: false,

                possible_matches: Vec::new(),
            })
            .collect(),
    }
}

fn event(pts: i64, participant_id: u64, kind: ChangeKind) -> ChangeEvent {
    ChangeEvent {
        timestamp: time(pts),
        participant_id,
        appearance_id: participant_id,
        kind,
        uncertain: false,
    }
}

fn analysis(observations: Vec<FrameObservation>, events: Vec<ChangeEvent>) -> AnalysisResult {
    AnalysisResult {
        schema_version: RESULT_SCHEMA_VERSION,
        start: time(-100),
        end: time(1000),
        sampling: Sampling::EveryFrame,
        observations,
        tracks: Vec::new(),
        appearances: Vec::new(),
        events,
        coverage: vec![CoverageInterval {
            start: time(-100),
            end: time(1000),
            status: Coverage::Observed,
        }],
        diagnostics: Vec::new(),
        resume_at: None,
    }
}

fn rule(unit: MarkerUnit, threshold: MarkerThreshold, kinds: &[ChangeKind]) -> MarkerRule {
    MarkerRule {
        unit,
        threshold,
        kinds: kinds.to_vec(),
        roster: None,
        settling_window_ms: 75,
    }
}

#[test]
fn swap_counts_distinct_people_and_requeries_against_the_selected_roster() {
    let result = analysis(
        vec![frame(0, &[1, 2, 3, 4]), frame(100, &[2, 1, 3, 4])],
        vec![
            event(0, 1, ChangeKind::Enter),
            event(100, 1, ChangeKind::Move),
            event(100, 1, ChangeKind::Resize),
            event(100, 2, ChangeKind::Move),
        ],
    );
    let mut rule = rule(
        MarkerUnit::Participants,
        MarkerThreshold::AtLeast(2),
        &[ChangeKind::Enter, ChangeKind::Move, ChangeKind::Resize],
    );
    let markers = result.scene_markers(&rule);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].timestamp, time(100));
    assert_eq!(markers[0].changed_count, 2);
    assert_eq!(markers[0].eligible_count, 4);
    assert_eq!(markers[0].participant_ids, [1, 2]);
    assert!(!markers[0].uncertain);

    rule.threshold = MarkerThreshold::All;
    assert!(result.scene_markers(&rule).is_empty());
    rule.roster = Some(vec![1, 2]);
    assert_eq!(result.scene_markers(&rule).len(), 1);
    rule.roster = Some(vec![]);
    assert!(result.scene_markers(&rule).is_empty());

    rule.unit = MarkerUnit::Slots;
    rule.roster = None;
    rule.threshold = MarkerThreshold::AtLeast(2);
    assert_eq!(result.scene_markers(&rule)[0].changed_count, 2);
}

#[test]
fn same_slot_replacement_counts_once_and_a_reflow_has_no_comparable_slots() {
    let result = analysis(
        vec![
            frame(0, &[1, 2]),
            frame(100, &[3, 2]),
            frame(300, &[3, 2, 4]),
        ],
        vec![
            event(100, 1, ChangeKind::Leave),
            event(100, 3, ChangeKind::Enter),
            event(300, 4, ChangeKind::Enter),
        ],
    );
    let mut rule = rule(
        MarkerUnit::Slots,
        MarkerThreshold::Any,
        &[ChangeKind::Enter, ChangeKind::Leave],
    );
    let markers = result.scene_markers(&rule);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].changed_count, 1);
    assert_eq!(markers[0].eligible_count, 2);

    rule.threshold = MarkerThreshold::All;
    rule.roster = Some(vec![1, 3]);
    let markers = result.scene_markers(&rule);
    assert_eq!(markers[0].eligible_count, 1);

    rule.unit = MarkerUnit::Participants;
    let markers = result.scene_markers(&rule);
    assert_eq!(markers[0].changed_count, 2);
    assert_eq!(markers[0].eligible_count, 2);
}

#[test]
fn settling_window_uses_exact_source_times_and_marks_unobserved_coverage() {
    let mut result = analysis(
        vec![
            frame(0, &[1, 2]),
            frame(40, &[1, 2]),
            frame(115, &[1, 2]),
            frame(116, &[1, 2]),
        ],
        vec![
            event(40, 1, ChangeKind::CameraState),
            ChangeEvent {
                timestamp: Timestamp::new(23, 1, 200),
                ..event(115, 2, ChangeKind::HardCut)
            },
            event(116, 2, ChangeKind::CameraState),
        ],
    );
    result.coverage = vec![
        CoverageInterval {
            start: time(-100),
            end: time(41),
            status: Coverage::Observed,
        },
        CoverageInterval {
            start: time(41),
            end: time(115),
            status: Coverage::Inferred,
        },
        CoverageInterval {
            start: time(115),
            end: time(1000),
            status: Coverage::Observed,
        },
    ];
    let mut rule = rule(
        MarkerUnit::Participants,
        MarkerThreshold::AtLeast(2),
        &[ChangeKind::CameraState, ChangeKind::HardCut],
    );
    let markers = result.scene_markers(&rule);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].timestamp, time(40));
    assert_eq!(
        markers[0].end_timestamp.compare(time(115)),
        Some(std::cmp::Ordering::Equal)
    );
    assert!(markers[0].uncertain);

    rule.settling_window_ms = 74;
    assert!(result.scene_markers(&rule).is_empty());
}

#[test]
fn tracker_emits_a_selectable_hard_cut_without_treating_a_border_flash_as_one() {
    const WIDTH: usize = 128;
    let picture = |inverted: bool, border: bool| {
        let mut data = Vec::new();
        for y in 0..WIDTH {
            for x in 0..WIDTH {
                let signal = |axis| {
                    let phase = axis % 64;
                    match phase {
                        0..4 => -1.0 + phase as f32 / 2.0,
                        4..32 => 1.0,
                        32..36 => 1.0 - (phase - 32) as f32 / 2.0,
                        _ => -1.0,
                    }
                };
                let value = signal(x) * signal(y) * if inverted { -24.0 } else { 24.0 };
                if border && (x == 0 || y == 0 || x == WIDTH - 1 || y == WIDTH - 1) {
                    data.extend([4, 220, 5]);
                } else {
                    data.extend([190.0, 80.0, 140.0].map(|channel| (channel + value) as u8));
                }
            }
        }
        data
    };

    let mut analyzer = Analyzer::new(
        AnalyzerConfig {
            mode: BackendMode::CpuOnly,
            sampling: Sampling::EveryFrame,
        },
        time(0),
    )
    .unwrap();
    for (index, data) in [
        picture(false, false),
        picture(false, true),
        picture(true, false),
    ]
    .iter()
    .enumerate()
    {
        analyzer
            .push(Frame {
                data,
                width: WIDTH as u32,
                height: WIDTH as u32,
                stride: WIDTH * 3,
                format: PixelFormat::Rgb24,
                timestamp: time(index as i64 * 40),
                display: DisplayGeometry::default(),
            })
            .unwrap();
    }
    let result = analyzer.finish(time(120)).unwrap();
    let rule = rule(
        MarkerUnit::Participants,
        MarkerThreshold::Any,
        &[ChangeKind::HardCut],
    );
    let markers = result.scene_markers(&rule);
    assert_eq!(markers.len(), 1, "events: {:?}", result.events);
    assert_eq!(markers[0].timestamp, time(80));
    assert!(markers[0].uncertain);
}

#[test]
fn marker_grouping_handles_full_timestamp_range_and_unordered_events() {
    let stamp = |pts| Timestamp::new(pts, u32::MAX, u32::MAX);
    let first = stamp(i64::MIN + 1);
    let boundary = stamp(i64::MIN + 2);
    let last = stamp(i64::MAX - 1);
    let mut result = analysis(
        vec![frame(0, &[1, 2]); 4],
        vec![
            ChangeEvent {
                timestamp: last,
                ..event(0, 1, ChangeKind::HardCut)
            },
            ChangeEvent {
                timestamp: boundary,
                ..event(0, 2, ChangeKind::HardCut)
            },
            ChangeEvent {
                timestamp: first,
                ..event(0, 1, ChangeKind::HardCut)
            },
            event(0, 2, ChangeKind::HardCut),
            ChangeEvent {
                timestamp: Timestamp::new(0, 0, 1),
                ..event(0, 2, ChangeKind::HardCut)
            },
        ],
    );
    result.start = stamp(i64::MIN);
    result.end = stamp(i64::MAX);
    result.coverage[0].start = result.start;
    result.coverage[0].end = result.end;
    for (frame, timestamp) in
        result
            .observations
            .iter_mut()
            .zip([result.start, first, boundary, last])
    {
        frame.timestamp = timestamp;
    }
    result.observations[1].regions[0].identity_uncertain = true;
    let mut rule = rule(
        MarkerUnit::Participants,
        MarkerThreshold::Any,
        &[ChangeKind::HardCut],
    );
    rule.settling_window_ms = 1000;
    let markers = result.scene_markers(&rule);
    assert_eq!(markers.len(), 2);
    assert_eq!(markers[0].timestamp, first);
    assert_eq!(markers[0].end_timestamp, boundary);
    assert_eq!(markers[0].participant_ids, [1, 2]);
    assert!(markers[0].uncertain);
    assert_eq!(markers[1].timestamp, last);
    assert!(!markers[1].uncertain);

    result.observations.swap(1, 2);
    assert!(result.scene_markers(&rule).is_empty());
}

#[test]
fn slots_accept_equivalent_pixel_aspect_ratios() {
    let mut result = analysis(
        vec![frame(0, &[1, 2]), frame(100, &[2, 1])],
        vec![
            event(100, 1, ChangeKind::Move),
            event(100, 2, ChangeKind::Move),
        ],
    );
    result.observations[1].display.pixel_aspect_num = u32::MAX;
    result.observations[1].display.pixel_aspect_den = u32::MAX;
    let rule = rule(MarkerUnit::Slots, MarkerThreshold::All, &[ChangeKind::Move]);
    assert_eq!(result.scene_markers(&rule)[0].changed_count, 2);
}

#[test]
fn queries_respect_scan_gaps_resume_and_half_open_intersections() {
    let mut result = analysis(vec![frame(0, &[1]), frame(100, &[1])], vec![]);
    result.coverage[0].start = time(0);
    result.coverage[0].end = time(50);
    result.coverage.push(CoverageInterval {
        start: time(75),
        end: time(1000),
        status: Coverage::Observed,
    });
    result.resume_at = Some(time(175));
    result.appearances = vec![
        Appearance {
            id: 1,
            participant_id: Some(1),
            start: time(0),
            end: time(175),
        },
        Appearance {
            id: 2,
            participant_id: Some(1),
            start: time(20),
            end: time(20),
        },
        Appearance {
            id: 3,
            participant_id: Some(1),
            start: time(175),
            end: time(225),
        },
    ];
    assert_eq!(result.coverage_at(time(-1)), None);
    assert_eq!(result.coverage_at(time(50)), None);
    assert_eq!(result.coverage_at(time(75)), Some(Coverage::Observed));
    assert!(result.visible_at(time(75)).is_none());
    assert_eq!(result.visible_at(time(100)).unwrap().timestamp, time(100));
    assert_eq!(result.coverage_at(time(175)), Some(Coverage::Incomplete));
    assert!(result.visible_at(time(175)).is_none());
    assert_eq!(result.coverage_at(time(1000)), None);
    let ids: Vec<_> = result
        .appearances_of(1, time(-200), time(1200))
        .map(|a| a.id)
        .collect();
    assert_eq!(ids, [1]);
    assert_eq!(result.appearances_of(1, time(175), time(200)).count(), 0);

    result.start = Timestamp::new(0, 0, 1000);
    assert_eq!(result.coverage_at(time(100)), None);
    assert_eq!(result.appearances_of(1, time(0), time(200)).count(), 0);
}
