# frozen_string_literal: true

require 'minitest/autorun'
require 'journal_machines'

# What every test file opens a journal with.
module JournalHelpers
  J = JournalMachines

  # Stand-in with the two readers of OffsetMachines::Tracker the journal uses.
  Tracker = Data.define(:generation, :base)

  # Test defaults only: the gem itself has none, like the core.
  def open_journal(base_offset: 0, now: 0, max_bytes: nil, max_records: nil, max_age: nil, min_dead_per_mille: 500)
    J::Journal.new(J::Policy.new(max_bytes, max_records, max_age, min_dead_per_mille), base_offset, now)
  end

  # A journal with `count` sealed ten-record, hundred-byte segments.
  def journal_with(count, **)
    journal = open_journal(**)
    [journal, count.times.map { journal.roll(100, 10, 0).sealed }]
  end
end
