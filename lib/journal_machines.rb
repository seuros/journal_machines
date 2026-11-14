# frozen_string_literal: true

require_relative 'journal_machines/version'

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
  # Integer nanoseconds on the monotonic clock the journal was given. Like the
  # core's `Segment::checked`, it holds only what some journal could answer:
  # anything else raises RangeError (TypeError for a wrong type: Integers,
  # a state Symbol, true or false, a Replacement or nil).
  Segment = Data.define(
    :id, :revision, :base_offset, :records, :stored_records, :live_records,
    :bytes, :opened_at, :state, :pinned, :replacement
  ) do
    def initialize(id:, revision:, base_offset:, records:, stored_records:, live_records:, bytes:, opened_at:,
                   state:, pinned:, replacement:)
      Journal.__send__(:__check_segment, [id, revision, base_offset, records],
                       [stored_records, live_records, bytes, opened_at], state, pinned, replacement)
      super
    end

    def end_offset = base_offset + records
  end
  # Journal#roll_decision when a threshold was reached: the core's
  # `Roll::Due(reason)`. `reason` is :size, :records or :age, checked in that
  # order. The other answer, `Roll::Keep`, is the symbol :keep. Any other
  # Symbol raises RangeError, a non-Symbol TypeError.
  Due = Data.define(:reason) do
    def initialize(reason:)
      Journal.__send__(:__check_due, reason)
      super
    end
  end
  # A recorded roll: the sealed segment, the new active one and its first
  # offset. Like the core's `Rolled::new`: two issued ids of one scope, the
  # opened one drawn after the sealed one, and a positive next offset, or
  # RangeError (TypeError for a non-Integer).
  Rolled = Data.define(:sealed, :opened, :next_offset) do
    def initialize(sealed:, opened:, next_offset:)
      Journal.__send__(:__check_rolled, sealed, opened, next_offset)
      super
    end
  end
  # Permission to delete one original segment file. Like the core's
  # `Reclaimed::new`: an issued segment id, a revision of 0 or an attempt drawn
  # after it, and a replaced_by (nil or Integer) later still, or RangeError
  # (TypeError for a non-Integer).
  Reclaimed = Data.define(:segment, :revision, :bytes, :replaced_by) do
    def initialize(segment:, revision:, bytes:, replaced_by:)
      Journal.__send__(:__check_reclaimed, segment, revision, bytes, replaced_by)
      super
    end
  end

end

# Native is required: no algorithm fallback. It resolves the Data values above
# once, so they must exist first.
require 'journal_machines_native/journal_machines_native'

module JournalMachines
  # Pin and Replacement handles are values: two handles naming the same pin
  # acquisition or replacement attempt are == and eql?, and hash alike.

  # The values are final. `initialize` is where each validates, and a subclass
  # could override it, or `allocate` build an instance that never ran it:
  # subclassing raises TypeError and `allocate` is gone. That stops accidents,
  # not reflection (Ruby binds a named subclass's constant before `inherited`
  # raises; `Marshal.load` and a hand-bound `Data#initialize` skip the check),
  # so the extension is the authority: the one value it reads back, a Policy,
  # must be exactly a Policy and is checked again by the core.
  [Policy, Segment, Due, Rolled, Reclaimed].each do |value|
    value.singleton_class.undef_method(:allocate)
    value.define_singleton_method(:inherited) { |_| raise TypeError, "#{value} cannot be subclassed" }
  end

  class Journal
    private_class_method :__open, :__check_policy, :__check_due, :__check_rolled, :__check_reclaimed,
                         :__check_segment

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
    # `policy` must be exactly a Policy (TypeError otherwise); the extension
    # reads its fields and checks them again.
    def self.resumed(policy, base_offset, now, scope, issued) = __open(base_offset, now, [scope, issued], policy)

    def policy = Policy.new(*__policy)

    def segment_capacity = SEGMENT_CAPACITY

    def pin_capacity = PIN_CAPACITY

    # The binding builds every answer (Segment, Rolled, Reclaimed, Due) after
    # the core call, from what the core returned; #roll and #reclaim decide on
    # a copy and keep it only once the answer is built and the journal is
    # still unfrozen and unchanged (FrozenError, RuntimeError otherwise, with
    # nothing applied). #segments,
    # #compaction_candidates and #reclaimable answer at most `limit` segments
    # after segment id `after` (nil to begin), in ascending id order; resume by
    # passing the last id answered. An `after` this journal never issued
    # (another scope, sequence zero, not reached) is no position: [].

    # Pin a checkpoint at a tracker's confirmed-committed base, under its
    # assignment generation. Both come from the one tracker passed.
    def pin_checkpoint_from(tracker) = pin_checkpoint(*checkpoint_source(tracker))

    # Move a checkpoint pin to a tracker's confirmed-committed base.
    def checkpoint_from(pin, tracker) = checkpoint(pin, *checkpoint_source(tracker))

    private :__policy

    private

    # Assignment generation and committed base of one offset tracker.
    def checkpoint_source(tracker)
      unless tracker.respond_to?(:generation) && tracker.respond_to?(:base)
        raise TypeError, 'expected an offset tracker responding to #generation and #base'
      end

      [tracker.generation, tracker.base]
    end
  end
end
