# frozen_string_literal: true

require_relative 'journal_machines/version'
# Native is required: no algorithm fallback.
require 'journal_machines_native/journal_machines_native'

module JournalMachines
  # Immutable point-in-time view of one segment's metadata.
  Segment = Data.define(
    :id, :revision, :base_offset, :records, :stored_records, :live_records,
    :bytes, :opened_at, :state, :pinned, :replacement
  ) do
    def end_offset = base_offset + records
  end
  # A recorded roll: the sealed segment, the new active one and its first offset.
  Rolled = Data.define(:sealed, :opened, :next_offset)
  # Permission to delete one original segment file.
  Reclaimed = Data.define(:segment, :revision, :bytes, :replaced_by)

  class Journal
    private_class_method :__open

    def self.new(base_offset: 0, now: 0, max_bytes: nil, max_records: nil, max_age: nil, min_dead_per_mille: 500)
      __open(base_offset, now, max_bytes, max_records, max_age, min_dead_per_mille)
    end

    def capacity = SEGMENT_CAPACITY

    def pin_capacity = PIN_CAPACITY

    def roll(bytes, records, now)
      rolled = __roll(bytes, records, now)
      rolled.is_a?(Symbol) ? rolled : Rolled.new(*rolled)
    end

    def active = wrap(__active)

    def segment(id) = wrap(__segment(id))

    def covering(offset) = wrap(__covering(offset))

    def segments(after = nil, limit = SEGMENT_CAPACITY)
      __scan(:all, after, limit).map { wrap(it) }
    end

    def compaction_candidates(after = nil, limit = SEGMENT_CAPACITY)
      __scan(:candidates, after, limit).map { wrap(it) }
    end

    def reclaimable(after = nil, limit = SEGMENT_CAPACITY)
      __scan(:reclaimable, after, limit).map { wrap(it) }
    end

    def reclaim(replacement)
      reclaimed = __reclaim(replacement)
      reclaimed.is_a?(Symbol) ? reclaimed : Reclaimed.new(*reclaimed)
    end

    private

    def wrap(row) = row && Segment.new(*row)
  end
end
