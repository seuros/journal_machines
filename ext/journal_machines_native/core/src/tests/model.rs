//! A naive, growable model of the journal, written without sharing its code.
use std::vec::Vec;

use crate::*;

pub const SEGMENTS: usize = 5;
pub const PINS: usize = 3;
pub type Real = Journal<SEGMENTS, PINS>;

#[derive(Debug, Clone)]
pub struct ModelSegment {
    pub id: u64,
    pub revision: u64,
    pub base: u64,
    pub span: u64,
    pub stored: u64,
    pub live: u64,
    pub bytes: u64,
    pub opened: u64,
    pub sealed: bool,
    pub attempt: Option<u64>,
    pub durable: Option<Durable>,
}

#[derive(Debug, Clone)]
pub struct ModelPin {
    pub generation: u64,
    pub reader: bool,
    pub offset: u64,
    pub assignment: u64,
}

pub struct Model {
    pub limits: [u64; 3],
    pub per_mille: u64,
    pub segments: Vec<ModelSegment>,
    pub pins: Vec<ModelPin>,
    pub ids: u64,
    pub attempts: u64,
    pub generations: u64,
}

impl Model {
    pub fn open(id: u64, base: u64, opened: u64) -> ModelSegment {
        ModelSegment {
            id,
            revision: 0,
            base,
            span: 0,
            stored: 0,
            live: 0,
            bytes: 0,
            opened,
            sealed: false,
            attempt: None,
            durable: None,
        }
    }

    pub fn pinned(&self, segment: &ModelSegment) -> bool {
        segment.sealed
            && self
                .pins
                .iter()
                .any(|pin| pin.offset < segment.base + segment.span)
    }

    pub fn view(&self, segment: &ModelSegment) -> Segment {
        let pinned = self.pinned(segment);
        let state = match (segment.sealed, segment.attempt, segment.durable) {
            (false, ..) => SegmentState::Active,
            (true, None, _) => SegmentState::Sealed,
            (true, Some(_), None) => SegmentState::Proposed,
            (true, Some(_), Some(_)) if pinned => SegmentState::Durable,
            (true, Some(_), Some(_)) => SegmentState::Reclaimable,
        };
        Segment {
            id: SegmentId::from_raw(segment.id),
            revision: segment.revision,
            base_offset: segment.base,
            records: segment.span,
            stored_records: segment.stored,
            live_records: segment.live,
            bytes: segment.bytes,
            opened_at: segment.opened,
            state,
            pinned,
            replacement: segment.attempt.map(|attempt| Replacement {
                segment: SegmentId::from_raw(segment.id),
                attempt,
            }),
        }
    }

    pub fn views(&self) -> Vec<Segment> {
        self.segments
            .iter()
            .map(|segment| self.view(segment))
            .collect()
    }

    pub fn candidate(&self, view: &Segment) -> bool {
        let dead = view.stored_records - view.live_records;
        view.state == SegmentState::Sealed
            && dead > 0
            && dead * 1000 >= view.stored_records * self.per_mille
    }

    pub fn roll_decision(&self, bytes: u64, records: u64, now: u64) -> Result<Roll, RollError> {
        let active = self.segments.last().expect("active");
        if now < active.opened {
            return Err(RollError::TimeRegression);
        }
        if records == 0 {
            return Ok(Roll::Keep);
        }
        let seen = [bytes, records, now - active.opened];
        let reasons = [RollReason::Size, RollReason::Records, RollReason::Age];
        for index in 0..3 {
            if self.limits[index] != 0 && seen[index] >= self.limits[index] {
                return Ok(Roll::Due(reasons[index]));
            }
        }
        Ok(Roll::Keep)
    }

    pub fn roll(&mut self, bytes: u64, records: u64, now: u64) -> Result<Rolled, RollError> {
        let full = self.segments.len() == SEGMENTS;
        let active = self.segments.last_mut().expect("active");
        if records == 0 {
            return Err(RollError::Empty);
        }
        if now < active.opened {
            return Err(RollError::TimeRegression);
        }
        if full {
            return Err(RollError::Full);
        }
        active.span = records;
        active.stored = records;
        active.live = records;
        active.bytes = bytes;
        active.sealed = true;
        let (sealed, next_offset) = (active.id, active.base + records);
        self.ids += 1;
        self.segments.push(Self::open(self.ids, next_offset, now));
        Ok(Rolled {
            sealed: SegmentId::from_raw(sealed),
            opened: SegmentId::from_raw(self.ids),
            next_offset,
        })
    }

    pub fn find(&mut self, id: u64) -> Option<&mut ModelSegment> {
        self.segments.iter_mut().find(|segment| segment.id == id)
    }

    pub fn observe_live(&mut self, id: u64, live: u64) -> Observe {
        match self.find(id) {
            None => Observe::Unknown,
            Some(segment) if !segment.sealed => Observe::Active,
            Some(segment) if live > segment.stored => Observe::Exceeds,
            Some(segment) => {
                segment.live = live;
                Observe::Recorded
            }
        }
    }

    pub fn pin(&mut self, reader: bool, assignment: u64, offset: u64) -> Result<u64, PinError> {
        if !reader && assignment == 0 {
            return Err(PinError::Unassigned);
        }
        if self.pins.len() == PINS {
            return Err(PinError::Full);
        }
        self.generations += 1;
        self.pins.push(ModelPin {
            generation: self.generations,
            reader,
            offset,
            assignment,
        });
        Ok(self.generations)
    }

    pub fn move_pin(
        &mut self,
        generation: u64,
        reader: bool,
        assignment: u64,
        to: u64,
    ) -> PinUpdate {
        let Some(pin) = self
            .pins
            .iter_mut()
            .find(|pin| pin.generation == generation)
        else {
            return PinUpdate::Stale;
        };
        if pin.reader != reader {
            return PinUpdate::WrongKind;
        }
        if !reader {
            if assignment == 0 {
                return PinUpdate::Unassigned;
            }
            if assignment < pin.assignment {
                return PinUpdate::StaleAssignment;
            }
            if assignment > pin.assignment {
                pin.assignment = assignment;
                pin.offset = to;
                return PinUpdate::Reassigned;
            }
        }
        match to.cmp(&pin.offset) {
            core::cmp::Ordering::Less => PinUpdate::Regressed,
            core::cmp::Ordering::Equal => PinUpdate::Unchanged,
            core::cmp::Ordering::Greater => {
                pin.offset = to;
                PinUpdate::Advanced
            }
        }
    }

    pub fn release(&mut self, generation: u64) -> Release {
        let before = self.pins.len();
        self.pins.retain(|pin| pin.generation != generation);
        if self.pins.len() == before {
            Release::Stale
        } else {
            Release::Released
        }
    }

    pub fn propose(&mut self, id: u64) -> Result<u64, ProposeError> {
        let attempt = self.attempts + 1;
        let segment = self.find(id).ok_or(ProposeError::Unknown)?;
        if !segment.sealed {
            return Err(ProposeError::Active);
        }
        if segment.attempt.is_some() {
            return Err(ProposeError::Busy);
        }
        segment.attempt = Some(attempt);
        self.attempts = attempt;
        Ok(attempt)
    }

    pub fn current(&mut self, handle: Replacement) -> Option<&mut ModelSegment> {
        self.find(handle.segment.raw())
            .filter(|segment| segment.attempt == Some(handle.attempt))
    }

    pub fn confirm(&mut self, handle: Replacement, durable: Durable) -> Confirm {
        let Some(segment) = self.current(handle) else {
            return Confirm::Stale;
        };
        if segment.durable.is_some() {
            return Confirm::AlreadyDurable;
        }
        if let Durable::Rewritten { live_records, .. } = durable
            && live_records > segment.stored
        {
            return Confirm::Invalid;
        }
        segment.durable = Some(durable);
        Confirm::Confirmed
    }

    pub fn abort(&mut self, handle: Replacement) -> Abort {
        let Some(segment) = self.current(handle) else {
            return Abort::Stale;
        };
        segment.attempt = None;
        segment.durable = None;
        Abort::Aborted
    }

    pub fn reclaim(&mut self, handle: Replacement) -> Result<Reclaimed, ReclaimError> {
        let lowest = self.pins.iter().map(|pin| pin.offset).min();
        let segment = self.current(handle).ok_or(ReclaimError::Stale)?;
        let durable = segment.durable.ok_or(ReclaimError::NotDurable)?;
        if let Some(offset) = lowest.filter(|offset| *offset < segment.base + segment.span) {
            return Err(ReclaimError::Pinned { offset });
        }
        let mut reclaimed = Reclaimed {
            segment: handle.segment,
            revision: segment.revision,
            bytes: segment.bytes,
            replaced_by: None,
        };
        if let Durable::Rewritten {
            bytes,
            live_records,
        } = durable
        {
            segment.revision = handle.attempt;
            segment.bytes = bytes;
            segment.stored = live_records;
            segment.live = segment.live.min(live_records);
            segment.attempt = None;
            segment.durable = None;
            reclaimed.replaced_by = Some(handle.attempt);
        } else {
            let id = segment.id;
            self.segments.retain(|segment| segment.id != id);
        }
        Ok(reclaimed)
    }
}
