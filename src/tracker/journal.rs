use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Sample, Tracker, evidence::Steadiness};
use crate::{
    Appearance, AppearanceId, ChangeEvent, FrameObservation, ParticipantId, ParticipantTrack,
};

/// Observations and events only append between committed pushes. The remaining
/// tracker state can change in place and is stored as of the latest push.
#[derive(Serialize)]
pub(crate) struct Chunk<'a> {
    observations: &'a [FrameObservation],
    events: &'a [ChangeEvent],
    tracks: &'a [ParticipantTrack],
    appearances: &'a [Appearance],
    active: &'a [Sample],
    dormant: &'a BTreeMap<ParticipantId, Sample>,
    uncertain_gap: bool,
    has_baseline: bool,
    steady: &'a BTreeMap<AppearanceId, Steadiness>,
}

#[derive(Deserialize)]
pub(crate) struct SavedChunk {
    observations: Vec<FrameObservation>,
    events: Vec<ChangeEvent>,
    tracks: Vec<ParticipantTrack>,
    appearances: Vec<Appearance>,
    active: Vec<Sample>,
    dormant: BTreeMap<ParticipantId, Sample>,
    uncertain_gap: bool,
    has_baseline: bool,
    steady: BTreeMap<AppearanceId, Steadiness>,
}

impl Tracker {
    pub(crate) fn journal_chunk(&self, observations: usize, events: usize) -> Chunk<'_> {
        Chunk {
            observations: &self.observations[observations..],
            events: &self.events[events..],
            tracks: &self.tracks,
            appearances: &self.appearances,
            active: &self.active,
            dormant: &self.dormant,
            uncertain_gap: self.uncertain_gap,
            has_baseline: self.has_baseline,
            steady: &self.steady,
        }
    }

    pub(crate) fn append_journal_chunk(&mut self, chunk: SavedChunk) {
        let SavedChunk {
            observations,
            events,
            tracks,
            appearances,
            active,
            dormant,
            uncertain_gap,
            has_baseline,
            steady,
        } = chunk;
        self.observations.extend(observations);
        self.events.extend(events);
        self.tracks = tracks;
        self.appearances = appearances;
        self.active = active;
        self.dormant = dormant;
        self.uncertain_gap = uncertain_gap;
        self.has_baseline = has_baseline;
        self.steady = steady;
    }
}
