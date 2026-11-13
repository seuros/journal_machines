# frozen_string_literal: true

require_relative 'journal_machines/version'
# Native is required: no algorithm fallback.
require 'journal_machines_native/journal_machines_native'

module JournalMachines
  # Roll thresholds and the dead-record ratio that makes a segment worth
  # compacting. A nil threshold never triggers; `max_age` is a span in Integer
  # nanoseconds. Like the core's `Policy::new`, an invalid policy cannot be
  # built: a zero threshold or a ratio above 1000 raises RangeError.
  Policy = Data.define(:max_bytes, :max_records, :max_age, :min_dead_per_mille) do
    def initialize(max_bytes:, max_records:, max_age:, min_dead_per_mille:)
      Journal.__send__(:__check_policy, max_bytes, max_records, max_age, min_dead_per_mille)
      super
    end
  end
  # Immutable point-in-time view of one segment's metadata. `opened_at` is
  # Integer nanoseconds on the monotonic clock the journal was given.
  Segment = Data.define(
    :id, :revision, :base_offset, :records, :stored_records, :live_records,
    :bytes, :opened_at, :state, :pinned, :replacement
  ) do
    def end_offset = base_offset + records
  end
  # Journal#roll_decision when a threshold was reached: the core's
  # `Roll::Due(reason)`. `reason` is :size, :records or :age, checked in that
  # order. The other answer, `Roll::Keep`, is the symbol :keep.
  Due = Data.define(:reason)
  # A recorded roll: the sealed segment, the new active one and its first offset.
  Rolled = Data.define(:sealed, :opened, :next_offset)
  # Permission to delete one original segment file.
  Reclaimed = Data.define(:segment, :revision, :bytes, :replaced_by)

  class Journal
    REASONS = %i[size records age].freeze
    private_constant :REASONS
    private_class_method :__open, :__check_policy

    # One active segment at `base_offset`, opened at `now`: Integer nanoseconds
    # of a monotonic clock (Process.clock_gettime(Process::CLOCK_MONOTONIC,
    # :nanosecond)). Every later `now` passed to #roll_decision and #roll is
    # read on the same clock. The journal is in scope zero.
    def self.new(policy, base_offset, now) = with_scope(policy, base_offset, now, 0)

    # As Journal.new, with every segment id, pin generation and attempt number
    # the journal issues carrying `scope`: an Integer in 0..65_535. A journal
    # of another scope answers :stale to its pins and replacements and
    # :unknown to its segment ids. Give each journal its own scope.
    def self.with_scope(policy, base_offset, now, scope) = resumed(policy, base_offset, now, scope, 0)

    # As Journal.with_scope after a restart: the journal continues the one
    # sequence its ids, generations and attempts are drawn from after
    # `issued`, the #issued saved from the journal before it, so nothing that
    # journal handed out is issued again. Segments and pins are not restored.
    # RangeError when `issued` leaves no number for the active segment.
    def self.resumed(policy, base_offset, now, scope, issued)
      raise TypeError, 'expected a JournalMachines::Policy' unless policy.is_a?(Policy)

      __open(base_offset, now, [scope, issued], policy.deconstruct)
    end

    def policy = Policy.new(*__policy)

    def segment_capacity = SEGMENT_CAPACITY

    def pin_capacity = PIN_CAPACITY

    # :keep, a Due naming the threshold reached, or a refusal. Reads only.
    def roll_decision(bytes, records, now)
      decision = __roll_decision(bytes, records, now)
      REASONS.include?(decision) ? Due.new(decision) : decision
    end

    def roll(bytes, records, now)
      rolled = __roll(bytes, records, now)
      rolled.is_a?(Symbol) ? rolled : Rolled.new(*rolled)
    end

    def active = wrap(__active)

    def segment(id) = wrap(__segment(id))

    def covering(offset) = wrap(__covering(offset))

    # At most `limit` segments after segment id `after` (nil to begin), in
    # ascending id order. Resume by passing the last id answered.
    def segments(after, limit)
      __scan(:all, after, limit).map { wrap(it) }
    end

    def compaction_candidates(after, limit)
      __scan(:candidates, after, limit).map { wrap(it) }
    end

    def reclaimable(after, limit)
      __scan(:reclaimable, after, limit).map { wrap(it) }
    end

    def reclaim(replacement)
      reclaimed = __reclaim(replacement)
      reclaimed.is_a?(Symbol) ? reclaimed : Reclaimed.new(*reclaimed)
    end

    # Pin a checkpoint at a tracker's confirmed-committed base, under its
    # assignment generation. Both come from the one tracker passed.
    def pin_checkpoint_from(tracker) = pin_checkpoint(*checkpoint_source(tracker))

    # Move a checkpoint pin to a tracker's confirmed-committed base.
    def checkpoint_from(pin, tracker) = checkpoint(pin, *checkpoint_source(tracker))

    private :__policy, :__roll_decision, :__roll, :__active, :__segment, :__covering, :__scan, :__reclaim

    private

    def wrap(row) = row && Segment.new(*row)

    # Assignment generation and committed base of one offset tracker.
    def checkpoint_source(tracker)
      unless tracker.respond_to?(:generation) && tracker.respond_to?(:base)
        raise TypeError, 'expected an offset tracker responding to #generation and #base'
      end

      [tracker.generation, tracker.base]
    end
  end
end
