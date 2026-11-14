# frozen_string_literal: true

require 'minitest/autorun'
require 'journal_machines'

# What every test file opens a journal with.
module JournalHelpers
  J = JournalMachines

  # Stand-in with the two readers of OffsetMachines::Tracker the journal uses.
  Tracker = Data.define(:generation, :base)

  # Test defaults only: the gem itself has none, like the core.
  # Without a scope it is opened with Journal.new, which is scope zero.
  def open_journal(base_offset: 0, now: 0, max_bytes: nil, max_records: nil, max_age: nil, min_dead_per_mille: 500,
                   scope: nil)
    policy = J::Policy.new(max_bytes, max_records, max_age, min_dead_per_mille)
    return J::Journal.new(policy, base_offset, now) unless scope

    J::Journal.with_scope(policy, base_offset, now, scope)
  end

  # Everything a caller can read back from a journal. Segments compare
  # whole: a Replacement is a value, equal to any handle to the same attempt.
  def snapshot(journal, pins)
    [journal.size, journal.segments(nil, 64), journal.pins, journal.low_pin, pins.map { journal.pin_offset(it) }]
  end

  # A journal with `count` sealed ten-record, hundred-byte segments.
  def journal_with(count, **)
    journal = open_journal(**)
    [journal, count.times.map { journal.roll(100, 10, 0).sealed }]
  end
end
