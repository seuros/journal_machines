//! A bounded replay window: which recent records a reader can still be sent.
use core::num::NonZeroU64;

use crate::Refusal;

/// The newest records of a log the caller stores, bounded by `RECORDS`
/// records and an optional byte budget, and where a reader stands against
/// them.
///
/// The window holds each retained record's byte size and nothing else: the
/// caller stores the records, appends them here in order, and deletes what
/// an append evicts. Records sit at consecutive offsets from the base offset
/// the window starts at; [`ReplayWindow::floor`] is the oldest one retained
/// and [`ReplayWindow::next_offset`] the one the next append takes.
///
/// Equality compares what the window retains, not where its ring keeps it.
#[derive(Debug, Clone)]
pub struct ReplayWindow<const RECORDS: usize> {
    max_bytes: Option<NonZeroU64>,
    sizes: [u64; RECORDS],
    oldest: usize,
    len: usize,
    bytes: u128,
    next_offset: u64,
}

/// What an honoured [`ReplayWindow::append`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appended {
    offset: u64,
    floor: u64,
}

impl Appended {
    /// An append some window answers: `None` unless `floor <= offset + 1`
    /// (the record is retained, or evicted with everything before it).
    #[must_use]
    pub const fn new(offset: u64, floor: u64) -> Option<Self> {
        match offset.checked_add(1) {
            Some(next) if floor <= next => Some(Self { offset, floor }),
            Some(_) | None => None,
        }
    }

    /// The offset the record took.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// The oldest offset still retained; every record below it was evicted
    /// and may be deleted. `offset + 1` when the record itself was over the
    /// byte budget and left at once.
    #[must_use]
    pub const fn floor(&self) -> u64 {
        self.floor
    }
}

/// Where a reader cursor stands against a [`ReplayWindow`]. The cursor is the
/// offset of the next record the reader wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Every record from the cursor on is retained: replay them all.
    Covered,
    /// Records the reader never saw were evicted; replay resumes at the floor.
    Behind {
        /// Records between the cursor and the floor that are gone.
        missed: u64,
    },
    /// The cursor is past [`ReplayWindow::next_offset`]: no record was ever
    /// appended there.
    Ahead,
}

impl<const RECORDS: usize> ReplayWindow<RECORDS> {
    /// Records the window retains at most.
    pub const CAPACITY: usize = RECORDS;

    /// An empty window whose first record takes `base_offset`. A `max_bytes`
    /// of `None` bounds the window by `RECORDS` alone.
    #[must_use]
    pub const fn new(max_bytes: Option<NonZeroU64>, base_offset: u64) -> Self {
        const { assert!(RECORDS > 0, "a replay window retains at least one record") };
        Self {
            max_bytes,
            sizes: [0; RECORDS],
            oldest: 0,
            len: 0,
            bytes: 0,
            next_offset: base_offset,
        }
    }

    /// The window an adapter stored, rebuilt from the sizes of the records it
    /// retains (oldest first), the last of them at `next_offset - 1`.
    ///
    /// `None` for a window no sequence of appends from `base_offset` leaves:
    /// more than `RECORDS` sizes, a floor below `base_offset`, a retained
    /// total past `max_bytes`, or an eviction nothing explains. Without a
    /// byte budget only a full window evicts, so it retains exactly
    /// `min(RECORDS, next_offset - base_offset)` records; with one, a window
    /// that evicted is full, or its evicted record could have been over the
    /// budget (a budget below `u64::MAX`, or a retained total above zero).
    #[must_use]
    pub fn restored(
        max_bytes: Option<NonZeroU64>,
        base_offset: u64,
        next_offset: u64,
        sizes: &[u64],
    ) -> Option<Self> {
        if sizes.len() > RECORDS {
            return None;
        }
        let evicted = next_offset
            .checked_sub(base_offset)?
            .checked_sub(u64::try_from(sizes.len()).ok()?)?;
        let mut window = Self::new(max_bytes, base_offset);
        for (slot, &size) in window.sizes.iter_mut().zip(sizes) {
            *slot = size;
            window.bytes += u128::from(size);
        }
        window.len = sizes.len();
        window.next_offset = next_offset;
        let explained = evicted == 0 || window.len == RECORDS;
        let reachable = max_bytes.map_or(explained, |budget| {
            window.bytes <= u128::from(budget.get())
                && (explained || budget.get() < u64::MAX || window.bytes > 0)
        });
        reachable.then_some(window)
    }

    /// Records that `bytes` more were appended at [`ReplayWindow::next_offset`],
    /// then evicts the oldest records until at most `RECORDS` remain and
    /// their sizes fit the byte budget. A record larger than the budget is
    /// evicted at once, with everything before it.
    ///
    /// # Errors
    /// [`Refusal::Overflow`] when the offset range is spent (`next_offset`
    /// is `u64::MAX`); the window is unchanged.
    pub fn append(&mut self, bytes: u64) -> Result<Appended, Refusal> {
        let offset = self.next_offset;
        let next = offset.checked_add(1).ok_or(Refusal::Overflow)?;
        if self.len == RECORDS {
            self.evict();
        }
        let tail = (self.oldest + self.len) % RECORDS;
        if let Some(slot) = self.sizes.get_mut(tail) {
            *slot = bytes;
            self.len += 1;
            self.bytes += u128::from(bytes);
        }
        self.next_offset = next;
        if let Some(budget) = self.max_bytes {
            while self.bytes > u128::from(budget.get()) && self.len > 0 {
                self.evict();
            }
        }
        Ok(Appended {
            offset,
            floor: self.floor(),
        })
    }

    /// Drops the oldest retained record.
    fn evict(&mut self) {
        if let Some(&size) = self.sizes.get(self.oldest) {
            self.bytes -= u128::from(size);
            self.len -= 1;
            self.oldest = (self.oldest + 1) % RECORDS;
        }
    }

    /// Where a reader whose next wanted record is `cursor` stands. Reads only.
    #[must_use]
    pub const fn coverage(&self, cursor: u64) -> Coverage {
        let floor = self.floor();
        if cursor > self.next_offset {
            Coverage::Ahead
        } else if cursor < floor {
            Coverage::Behind {
                missed: floor - cursor,
            }
        } else {
            Coverage::Covered
        }
    }

    /// The oldest retained offset; [`ReplayWindow::next_offset`] when the
    /// window retains nothing.
    #[must_use]
    pub const fn floor(&self) -> u64 {
        self.next_offset - self.len as u64
    }

    /// The offset the next append takes.
    #[must_use]
    pub const fn next_offset(&self) -> u64 {
        self.next_offset
    }

    /// Records retained.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether no record is retained.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bytes retained: the sum of the retained records' sizes, exact.
    #[must_use]
    pub const fn bytes(&self) -> u128 {
        self.bytes
    }

    /// The byte budget, `None` when only `RECORDS` bounds the window.
    #[must_use]
    pub const fn max_bytes(&self) -> Option<NonZeroU64> {
        self.max_bytes
    }

    /// The size of the retained record at `offset`; `None` outside
    /// `floor..next_offset`.
    #[must_use]
    pub fn size(&self, offset: u64) -> Option<u64> {
        let age = usize::try_from(offset.checked_sub(self.floor())?).ok()?;
        if age >= self.len {
            return None;
        }
        self.sizes.get((self.oldest + age) % RECORDS).copied()
    }

    /// The retained sizes, oldest first.
    pub fn sizes(&self) -> impl Iterator<Item = u64> + '_ {
        (0..self.len).filter_map(|age| self.sizes.get((self.oldest + age) % RECORDS).copied())
    }
}

impl<const RECORDS: usize> PartialEq for ReplayWindow<RECORDS> {
    fn eq(&self, other: &Self) -> bool {
        self.max_bytes == other.max_bytes
            && self.next_offset == other.next_offset
            && self.len == other.len
            && self.sizes().eq(other.sizes())
    }
}

impl<const RECORDS: usize> Eq for ReplayWindow<RECORDS> {}
