# frozen_string_literal: true

require_relative 'test_helper'

# Every identity a journal issues carries its scope.
class ScopeTest < Minitest::Test
  include JournalHelpers

  Issued = Data.define(:journal, :sealed, :reader, :checkpoint, :released, :proposed, :durable)

  # Everything one journal hands out, built by the same calls in any scope.
  def issued(scope)
    journal, sealed = journal_with(2, scope:)
    released = journal.pin_reader(30)
    journal.release(released)
    reader = journal.pin_reader(30)
    checkpoint = journal.pin_checkpoint(3, 30)
    proposed = journal.propose(sealed[0])
    durable = journal.propose(sealed[1])
    journal.confirm_superseded(durable)
    Issued.new(journal, sealed[0], reader, checkpoint, released, proposed, durable)
  end

  def snapshot(issued)
    journal = issued.journal
    rows = journal.segments(nil, 64).map { |s| [*s.to_h.except(:replacement).values, s.replacement&.attempt] }
    [rows, journal.pins, journal.low_pin, journal.pin_offset(issued.reader), journal.pin_offset(issued.checkpoint)]
  end

  def test_new_is_scope_zero_and_issues_one_plain_counter
    zero = issued(nil)
    assert_equal 0, zero.journal.scope
    assert_equal [1, 3], [zero.sealed, zero.journal.active.id]
    assert_equal [4, 5, 6], [zero.released, zero.reader, zero.checkpoint].map(&:generation)
    assert_equal [7, 8], [zero.proposed, zero.durable].map(&:attempt)
    assert_equal [0, 0], [zero.reader.scope, zero.durable.scope]
    assert_equal 8, zero.journal.issued
  end

  # Regression: ids, generations and attempts used to count separately, so in
  # scope 3 the first of each was the same number and an attempt (also a
  # revision) passed as a segment id named a real segment.
  def test_no_two_handles_of_a_journal_share_a_number
    all = issued(3)
    journal = all.journal
    numbers = [all.sealed, all.durable.segment, journal.active.id,
               *[all.released, all.reader, all.checkpoint].map(&:generation),
               *[all.proposed, all.durable].map(&:attempt)]
    assert_equal numbers.uniq, numbers
    assert_equal [3], numbers.map { it >> 48 }.uniq
    before = snapshot(all)
    [all.proposed.attempt, all.durable.attempt, all.reader.generation].each do |number|
      assert_nil journal.segment(number)
      assert_equal %i[unknown unknown], [journal.propose(number), journal.observe_live(number, 0)]
    end
    assert_equal before, snapshot(all)
  end

  def test_with_scope_stamps_every_identity
    high = issued(9)
    stamp = 9 << 48
    assert_equal 9, high.journal.scope
    assert_equal [stamp | 1, stamp | 3], [high.sealed, high.journal.active.id]
    assert_equal [stamp | 5, 9], [high.reader.generation, high.reader.scope]
    assert_equal [stamp | 2, stamp | 8, 9], [high.durable.segment, high.durable.attempt, high.durable.scope]
    assert_equal stamp | 7, high.journal.segment(high.sealed).replacement.attempt
    assert_equal 65_535, open_journal(scope: 65_535).scope
  end

  def test_a_handle_from_another_journal_names_nothing
    near = issued(1)
    far = issued(2)
    [[near, far], [far, near]].each do |own, other|
      before = snapshot(own)
      assert_equal [:stale], foreign_pin_answers(own.journal, other).uniq
      assert_equal [:stale], foreign_replacement_answers(own.journal, other).uniq
      assert_nil own.journal.pin_offset(other.reader)
      assert_nil own.journal.segment(other.sealed)
      assert_equal %i[unknown unknown], [own.journal.observe_live(other.sealed, 1), own.journal.propose(other.sealed)]
      scans = %i[segments compaction_candidates reclaimable].map { own.journal.public_send(it, other.sealed, 64) }
      assert_equal [[], [], []], scans
      assert_equal before, snapshot(own)
    end
    assert_equal :unchanged, near.journal.advance(near.reader, 30)
    assert_equal %i[stale released], [near.journal.release(near.released), near.journal.release(near.reader)]
    assert_equal :already_durable, near.journal.confirm_superseded(near.durable)
  end

  def foreign_pin_answers(journal, other)
    tracker = Tracker.new(3, 30)
    [journal.advance(other.reader, 30), journal.advance(other.reader, 31),
     journal.checkpoint(other.checkpoint, 3, 30), journal.checkpoint(other.checkpoint, 4, 0),
     journal.checkpoint_from(other.checkpoint, tracker),
     journal.release(other.reader), journal.release(other.released)]
  end

  def foreign_replacement_answers(journal, other)
    [journal.confirm_rewritten(other.proposed, 1, 1), journal.confirm_superseded(other.durable),
     journal.reclaim(other.durable), journal.reclaim(other.proposed),
     journal.abort(other.proposed), journal.abort(other.durable)]
  end

  def test_two_journals_left_in_one_scope_still_alias
    one = issued(5)
    other = issued(5)
    assert_equal :moved, one.journal.advance(other.reader, 31)
    assert_equal :released, one.journal.release(other.checkpoint)
    assert_equal :aborted, one.journal.abort(other.proposed)
  end

  def test_resumed_continues_the_sequence_and_reissues_nothing
    policy = J::Policy.new(nil, nil, nil, 500)
    [0, 9, 65_535].each do |scope|
      stamp = scope << 48
      fresh = J::Journal.resumed(policy, 0, 0, scope, 0)
      assert_equal [scope, 1, stamp | 1], [fresh.scope, fresh.issued, fresh.active.id]
      first = issued(scope)
      second = J::Journal.resumed(policy, 20, 0, scope, first.journal.issued)
      assert_equal [scope, 9, stamp | 9, 20], [second.scope, second.issued, second.active.id, second.active.base_offset]
      assert_equal [stamp | 10, stamp | 11], [second.roll(1, 1, 0).opened, second.pin_reader(0).generation]
      # What outlived the restart names nothing in the new incarnation.
      assert_equal [:stale], (foreign_pin_answers(second, first) + foreign_replacement_answers(second, first)).uniq
      assert_equal [nil, :unknown], [second.segment(first.sealed), second.propose(first.sealed)]
      assert_equal [2, 1], [second.size, second.pins]
    end
  end

  def test_a_journal_started_again_in_its_scope_reissues
    old = open_journal(scope: 9).pin_reader(5)
    again = open_journal(scope: 9)
    assert_equal old.generation, again.pin_checkpoint(1, 7).generation
    assert_equal :released, again.release(old)
  end

  def test_resumed_reaches_the_end_of_the_sequence
    policy = J::Policy.new(nil, nil, nil, 500)
    last = (2**48) - 1
    journal = J::Journal.resumed(policy, 0, 0, 65_535, last - 2)
    assert_equal (2**64) - 2, journal.active.id
    assert_equal [:unassigned, :empty, last - 1], [journal.pin_checkpoint(0, 0), journal.roll(1, 0, 0), journal.issued]
    pin = journal.pin_reader(0)
    assert_equal [(2**64) - 1, last], [pin.generation, journal.issued]
    assert_equal %i[exhausted exhausted exhausted],
                 [journal.roll(1, 1, 0), journal.pin_reader(0), journal.pin_checkpoint(1, 0)]
    assert_equal [last, 1, 1], [journal.issued, journal.size, journal.pins]
    assert_equal %i[released stale], [journal.release(pin), journal.release(pin)]
  end

  def test_resumed_checks_its_arguments
    policy = J::Policy.new(nil, nil, nil, 500)
    [(2**48) - 1, 2**48, (9 << 48) | 4, (2**64) - 1].each do |bad|
      assert_raises(RangeError) { J::Journal.resumed(policy, 0, 0, 9, bad) }
    end
    [-1, 2**64].each { |bad| assert_raises(RangeError) { J::Journal.resumed(policy, 0, 0, 9, bad) } }
    [1.0, nil, '1', Object.new].each { |bad| assert_raises(TypeError) { J::Journal.resumed(policy, 0, 0, 9, bad) } }
    [1.0, nil].each { |bad| assert_raises(TypeError) { J::Journal.resumed(policy, 0, 0, bad, 0) } }
    assert_raises(RangeError) { J::Journal.resumed(policy, 0, 0, 65_536, 0) }
    assert_raises(TypeError) { J::Journal.resumed(nil, 0, 0, 1, 0) }
    assert_raises(ArgumentError) { J::Journal.resumed(policy, 0, 0, 1) }
  end

  def test_scope_is_a_strict_integer
    policy = J::Policy.new(nil, nil, nil, 500)
    [1.0, nil, '1', 1r, Object.new].each { |bad| assert_raises(TypeError) { J::Journal.with_scope(policy, 0, 0, bad) } }
    [-1, 65_536, 2**64].each { |bad| assert_raises(RangeError) { J::Journal.with_scope(policy, 0, 0, bad) } }
    assert_raises(TypeError) { J::Journal.with_scope(nil, 0, 0, 1) }
    assert_raises(ArgumentError) { J::Journal.with_scope(policy, 0, 0) }
  end
end
