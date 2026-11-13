# frozen_string_literal: true

require_relative 'test_helper'

# Refusals, honoured repeats and the line between them.
class RefusalsTest < Minitest::Test
  include JournalHelpers

  # Regression: a pin the journal does not hold is :stale. It keeps no record
  # of the pins it issued, so it cannot prove a second release is a repeat.
  def test_releasing_a_pin_that_is_not_held_is_stale
    a = open_journal
    b = open_journal
    p = a.pin_checkpoint(1, 0)
    q = b.pin_reader(0)
    assert_equal :released, b.release(q)
    assert_equal :stale, b.release(p)
    assert_equal :stale, b.release(q)
    assert_equal [0, 1, 0], [b.pins, a.pins, a.pin_offset(p)]
  end

  # Regression: a repeat is a repeat only with the payload that was recorded.
  def test_confirming_a_durable_attempt_with_another_payload_is_a_mismatch
    journal = open_journal
    journal.roll(100, 10, 0)
    handle = journal.propose(1)
    assert_equal :recorded, journal.confirm_rewritten(handle, 50, 5)
    assert_equal :mismatch, journal.confirm_superseded(handle)
    assert_equal :mismatch, journal.confirm_rewritten(handle, 51, 5)
    assert_equal :mismatch, journal.confirm_rewritten(handle, 50, 4)
    assert_equal :mismatch, journal.confirm_rewritten(handle, 50, 11)
    assert_equal :already_durable, journal.confirm_rewritten(handle, 50, 5)
    assert_equal 1, journal.reclaim(handle).replaced_by
    assert_equal [50, 5], journal.segment(1).to_h.values_at(:bytes, :stored_records)

    journal.roll(100, 10, 0)
    gone = journal.propose(2)
    assert_equal :recorded, journal.confirm_superseded(gone)
    assert_equal :mismatch, journal.confirm_rewritten(gone, 50, 5)
    assert_equal :already_durable, journal.confirm_superseded(gone)
    assert_nil journal.reclaim(gone).replaced_by
    assert_nil journal.segment(2)
  end

  # Everything a caller can read back from a journal. Replacement handles are
  # fresh objects on every read, so they are compared by what they name.
  def snapshot(journal, pins)
    rows = journal.segments(nil, 64).map { |s| [*s.to_h.except(:replacement).values, s.replacement&.attempt] }
    [journal.size, rows, journal.pins, journal.low_pin, pins.map { journal.pin_offset(it) }]
  end

  def test_every_refusal_and_repeat_is_a_symbol_and_changes_nothing
    journal = open_journal(now: 5)
    3.times { journal.roll(100, 10, 5) }
    stale_pin = journal.pin_reader(0)
    journal.release(stale_pin)
    reader = journal.pin_reader(5)
    checkpoint = journal.pin_checkpoint(3, 5)
    stale = journal.propose(1)
    journal.abort(stale)
    proposed = journal.propose(1)
    durable = journal.propose(2)
    journal.confirm_superseded(durable)
    rewritten = journal.propose(3)
    journal.confirm_rewritten(rewritten, 1, 9)
    tracker = Tracker.new(3, 5)
    refusals = {
      empty: -> { journal.roll(1, 0, 5) },
      time_regression: -> { journal.roll(1, 1, 4) },
      overflow: -> { journal.roll(1, 2**64 - 1, 5) },
      unknown: -> { [journal.observe_live(9, 0), journal.propose(9)] },
      active: -> { [journal.observe_live(4, 0), journal.propose(4)] },
      exceeds: -> { [journal.observe_live(1, 11), journal.confirm_rewritten(proposed, 1, 11)] },
      mismatch: -> { [journal.confirm_superseded(rewritten), journal.confirm_rewritten(durable, 1, 1)] },
      busy: -> { [journal.propose(1), journal.propose(2)] },
      unassigned: lambda {
        [journal.pin_checkpoint(0, 1), journal.pin_checkpoint_from(Tracker.new(0, 1)),
         journal.checkpoint(checkpoint, 0, 9), journal.checkpoint_from(checkpoint, Tracker.new(0, 9))]
      },
      stale: lambda {
        [journal.advance(stale_pin, 9), journal.checkpoint(stale_pin, 3, 9), journal.release(stale_pin),
         journal.checkpoint_from(stale_pin, tracker), journal.confirm_superseded(stale),
         journal.confirm_rewritten(stale, 1, 1), journal.reclaim(stale), journal.abort(stale)]
      },
      wrong_kind: lambda {
        [journal.advance(checkpoint, 9), journal.checkpoint(reader, 3, 9), journal.checkpoint_from(reader, tracker)]
      },
      stale_assignment: lambda {
        [journal.checkpoint(checkpoint, 2, 9), journal.checkpoint_from(checkpoint, Tracker.new(2, 9))]
      },
      regressed: -> { [journal.advance(reader, 4), journal.checkpoint(checkpoint, 3, 4)] },
      not_durable: -> { journal.reclaim(proposed) },
      pinned: -> { journal.reclaim(durable) }
    }
    # Honoured idempotent repeats: what was asked for already holds.
    repeats = {
      unchanged: lambda {
        [journal.advance(reader, 5), journal.checkpoint(checkpoint, 3, 5), journal.checkpoint_from(checkpoint, tracker)]
      },
      already_durable: -> { [journal.confirm_superseded(durable), journal.confirm_rewritten(rewritten, 1, 9)] }
    }
    before = snapshot(journal, [stale_pin, reader, checkpoint])
    refusals.merge(repeats).each do |symbol, call|
      assert_equal [symbol], Array(call.call).uniq
      assert_equal before, snapshot(journal, [stale_pin, reader, checkpoint]), "#{symbol} must change nothing"
    end
  end

  def test_handles_belong_to_one_journal
    a, = journal_with(1)
    b, = journal_with(1)
    pin = a.pin_reader(0)
    replacement = a.propose(1)
    b.pin_reader(0)
    b.propose(1)
    assert_equal :stale, b.release(pin)
    assert_equal :stale, b.advance(pin, 5)
    assert_equal :stale, b.checkpoint(pin, 1, 5)
    assert_equal :stale, b.checkpoint_from(pin, Tracker.new(1, 5))
    assert_nil b.pin_offset(pin)
    assert_equal :stale, b.confirm_superseded(replacement)
    assert_equal :stale, b.confirm_rewritten(replacement, 1, 1)
    assert_equal :stale, b.reclaim(replacement)
    assert_equal :stale, b.abort(replacement)
    assert_raises(TypeError) { J::Pin.allocate }
    assert_raises(TypeError) { J::Replacement.allocate }
    assert_raises(TypeError) { J::Journal.allocate }
    assert_raises(NoMethodError) { J::Journal.__open(0, 0, nil, nil, nil, 0) }
    assert_raises(TypeError) { a.release(replacement) }
    assert_raises(TypeError) { a.abort(pin) }
  end
end
