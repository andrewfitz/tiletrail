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
