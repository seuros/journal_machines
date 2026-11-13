# frozen_string_literal: true

require_relative 'test_helper'

class JournalTest < Minitest::Test
  include JournalHelpers

  def test_capacities_are_compiled_constants
    journal = open_journal
    assert_equal [64, 32], [J::SEGMENT_CAPACITY, J::PIN_CAPACITY]
    assert_equal [64, 32], [journal.segment_capacity, journal.pin_capacity]
    assert_equal 1, journal.size
    refute_predicate journal, :empty?
  end

  def test_policy_mirrors_the_core_and_has_no_defaults
    policy = J::Policy.new(100, 10, 50, 250)
    assert_equal %i[max_bytes max_records max_age min_dead_per_mille], J::Policy.members
    assert_equal policy, J::Policy.new(max_bytes: 100, max_records: 10, max_age: 50, min_dead_per_mille: 250)
    journal = J::Journal.new(policy, 7, 9)
    assert_equal policy, journal.policy
    assert_equal [7, 9], journal.active.then { [it.base_offset, it.opened_at] }
    assert_equal J::Policy.new(nil, nil, nil, 0), J::Journal.new(J::Policy.new(nil, nil, nil, 0), 0, 0).policy
    assert_raises(ArgumentError) { J::Policy.new(100, 10, 50) }
    assert_raises(ArgumentError) { J::Policy.new(**policy.to_h, extra: 1) }
    [[0, nil, nil, 0], [nil, 0, nil, 0], [nil, nil, 0, 0], [nil, nil, nil, 1001], [-1, nil, nil, 0]].each do |fields|
      assert_raises(RangeError) { J::Policy.new(*fields) }
    end
    [[1.0, nil, nil, 0], [nil, '1', nil, 0], [nil, nil, 1r, 0], [nil, nil, nil, nil]].each do |fields|
      assert_raises(TypeError) { J::Policy.new(*fields) }
    end
    assert_raises(ArgumentError) { J::Journal.new }
    assert_raises(ArgumentError) { J::Journal.new(policy) }
    [nil, policy.to_h, [100, 10, 50, 250]].each { |v| assert_raises(TypeError) { J::Journal.new(v, 0, 0) } }
    %i[__policy __roll __active __segment __covering __scan __reclaim].each do |helper|
      assert_raises(NoMethodError) { journal.public_send(helper) }
    end
    assert_raises(NoMethodError) { J::Journal.__check_policy(nil, nil, nil, 0) }
  end

  def test_roll_decision_from_observations
    journal = open_journal(now: 1000, max_bytes: 100, max_records: 10, max_age: 50)
    assert_equal :keep, journal.roll_decision(99, 9, 1049)
    assert_equal JournalMachines::Due.new(:size), journal.roll_decision(100, 10, 1050)
    assert_equal JournalMachines::Due.new(:records), journal.roll_decision(1, 10, 1050)
    assert_equal JournalMachines::Due.new(:age), journal.roll_decision(1, 1, 1050)
    assert_equal :keep, journal.roll_decision(0, 0, 9999)
    assert_equal :time_regression, journal.roll_decision(1, 1, 999)
    assert_equal :keep, open_journal.roll_decision(2**64 - 1, 2**64 - 1, 2**64 - 1)
    assert_equal :overflow, open_journal(base_offset: 2**64 - 1).roll_decision(1, 1, 0)
  end

  def test_time_is_integer_nanoseconds
    second = 1_000_000_000
    now = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
    journal = open_journal(now:, max_age: 30 * second)
    assert_equal now, journal.active.opened_at
    assert_equal :keep, journal.roll_decision(1, 1, now + (30 * second) - 1)
    assert_equal JournalMachines::Due.new(:age), journal.roll_decision(1, 1, now + (30 * second))
    assert_equal now + 1, journal.roll(1, 1, now + 1).then { journal.active.opened_at }
    top = 2**64 - 1
    assert_equal top, open_journal(now: top, max_age: top).active.opened_at
    [1.0, 1r, '1', nil, Object.new].each do |v|
      assert_raises(TypeError) { open_journal(now: v) }
      assert_raises(TypeError) { journal.roll_decision(1, 1, v) }
      assert_raises(TypeError) { journal.roll(1, 1, v) }
    end
    [1.0, 1r, '1', Object.new].each { |v| assert_raises(TypeError) { open_journal(max_age: v) } }
    [-1, 2**64].each do |v|
      assert_raises(RangeError) { open_journal(max_age: v) }
      assert_raises(RangeError) { journal.roll_decision(1, 1, v) }
      assert_raises(RangeError) { journal.roll(1, 1, v) }
    end
  end

  def test_roll_records_the_seal
    journal = open_journal(base_offset: 100, now: 7)
    assert_equal :empty, journal.roll(64, 0, 7)
    assert_equal :time_regression, journal.roll(64, 5, 6)
    rolled = journal.roll(64, 5, 9)
    assert_instance_of J::Rolled, rolled
    assert_equal [1, 2, 105], [rolled.sealed, rolled.opened, rolled.next_offset]
    sealed = journal.segment(1)
    assert_instance_of J::Segment, sealed
    assert_predicate sealed, :frozen?
    assert_equal [:sealed, 100, 105, 64, 5, 5, 7, 0],
                 [sealed.state, sealed.base_offset, sealed.end_offset, sealed.bytes,
                  sealed.stored_records, sealed.live_records, sealed.opened_at, sealed.revision]
    assert_equal [2, :active, 105, 9], journal.active.to_h.values_at(:id, :state, :base_offset, :opened_at)
    assert_nil journal.segment(9)
    assert_nil journal.covering(99)
    assert_equal 1, journal.covering(104).id
    assert_equal 2, journal.covering(10**9).id
    assert_equal :overflow, journal.roll(1, 2**64 - 1, 9)
  end

  def test_full_journal_refuses_to_roll
    journal, sealed = journal_with(63)
    assert_equal 64, journal.size
    assert_equal :segments_full, journal.roll(1, 1, 0)
    assert_equal 64, journal.segments(nil, 64).size
    replacement = journal.propose(sealed.first)
    assert_equal :recorded, journal.confirm_superseded(replacement)
    assert_instance_of J::Reclaimed, journal.reclaim(replacement)
    assert_equal 66, journal.roll(1, 1, 0).opened, 'ids ascend, not consecutive: the attempt took 65'
  end

  def test_compaction_candidates_are_bounded_and_resumable
    journal, sealed = journal_with(4)
    assert_empty journal.compaction_candidates(nil, 64)
    assert_equal :recorded, journal.observe_live(sealed[0], 5)
    assert_equal :recorded, journal.observe_live(sealed[1], 6)
    assert_equal :recorded, journal.observe_live(sealed[2], 0)
    assert_equal :recorded, journal.observe_live(sealed[3], 1)
    assert_equal :exceeds, journal.observe_live(sealed[0], 11)
    assert_equal :active, journal.observe_live(5, 0)
    assert_equal :unknown, journal.observe_live(99, 0)
    assert_equal [1, 3, 4], journal.compaction_candidates(nil, 64).map(&:id)
    assert_equal [1, 3], journal.compaction_candidates(nil, 2).map(&:id)
    assert_equal [4], journal.compaction_candidates(3, 2).map(&:id)
    assert_empty journal.compaction_candidates(nil, 0)
    assert_equal [3, 4, 5], journal.segments(2, 64).map(&:id)
    assert_equal 5, journal.segments(nil, 10**6).size
    journal.propose(1)
    assert_equal [3, 4], journal.compaction_candidates(nil, 64).map(&:id)
    strict, = journal_with(1, min_dead_per_mille: 0)
    assert_empty strict.compaction_candidates(nil, 64)
    strict.observe_live(1, 9)
    assert_equal [1], strict.compaction_candidates(nil, 64).map(&:id)
  end

  def test_replacement_lifecycle
    journal, sealed = journal_with(2)
    assert_equal :active, journal.propose(3)
    assert_equal :unknown, journal.propose(9)
    replacement = journal.propose(sealed[0])
    assert_instance_of J::Replacement, replacement
    assert_equal [1, 4], [replacement.segment, replacement.attempt]
    assert_equal :busy, journal.propose(sealed[0])
    assert_equal :proposed, journal.segment(1).state
    assert_equal 4, journal.segment(1).replacement.attempt
    assert_equal :not_durable, journal.reclaim(replacement)
    assert_equal :exceeds, journal.confirm_rewritten(replacement, 1, 11)
    assert_equal :recorded, journal.confirm_rewritten(replacement, 30, 4)
    assert_equal :already_durable, journal.confirm_rewritten(replacement, 30, 4)
    assert_equal :mismatch, journal.confirm_superseded(replacement)
    assert_equal :mismatch, journal.confirm_rewritten(replacement, 1, 11)
    assert_equal :reclaimable, journal.segment(1).state
    assert_equal [1], journal.reclaimable(nil, 64).map(&:id)
    reclaimed = journal.reclaim(journal.reclaimable(nil, 64).first.replacement)
    assert_equal({ segment: 1, revision: 0, bytes: 100, replaced_by: 4 }, reclaimed.to_h)
    after = journal.segment(1)
    assert_equal [:sealed, 4, 30, 10, 4, 4, nil],
                 [after.state, after.revision, after.bytes, after.records,
                  after.stored_records, after.live_records, after.replacement]
    assert_equal :stale, journal.reclaim(replacement)
    assert_equal :stale, journal.abort(replacement)
    assert_equal :stale, journal.confirm_superseded(replacement)

    gone = journal.propose(sealed[1])
    assert_equal :recorded, journal.confirm_superseded(gone)
    assert_nil journal.reclaim(gone).replaced_by
    assert_equal [1, 3], journal.segments(nil, 64).map(&:id)
  end

  def test_abort_keeps_the_original
    journal, = journal_with(1)
    old = journal.propose(1)
    assert_equal :aborted, journal.abort(old)
    assert_equal :stale, journal.abort(old)
    assert_equal :sealed, journal.segment(1).state
    fresh = journal.propose(1)
    assert_equal :stale, journal.confirm_superseded(old)
    assert_equal :recorded, journal.confirm_superseded(fresh)
    assert_equal :aborted, journal.abort(fresh)
  end

  def test_reclaim_refused_while_any_pin_holds
    journal, = journal_with(2)
    reader = journal.pin_reader(3)
    checkpoint = journal.pin_checkpoint(1, 12)
    assert_equal %i[reader checkpoint], [reader.kind, checkpoint.kind]
    assert_equal [4, 5], [reader.generation, checkpoint.generation]
    assert_equal [2, 3], [journal.pins, journal.low_pin]
    first = journal.propose(1)
    second = journal.propose(2)
    [first, second].each { assert_equal :recorded, journal.confirm_superseded(it) }
    assert_equal [:durable, true], journal.segment(1).to_h.values_at(:state, :pinned)
    assert_empty journal.reclaimable(nil, 64)
    assert_equal :pinned, journal.reclaim(first)
    assert_equal :moved, journal.advance(reader, 9)
    assert_equal :pinned, journal.reclaim(first)
    assert_equal :moved, journal.advance(reader, 10)
    assert_equal [1], journal.reclaimable(nil, 64).map(&:id)
    assert_equal :pinned, journal.reclaim(second)
    assert_equal :released, journal.release(reader)
    assert_equal :pinned, journal.reclaim(second)
    assert_equal :advanced, journal.checkpoint(checkpoint, 1, 20)
    assert_instance_of J::Reclaimed, journal.reclaim(second)
    assert_instance_of J::Reclaimed, journal.reclaim(first)
    assert_equal [3], journal.segments(nil, 64).map(&:id)
  end

  def test_pins_are_generation_checked
    journal = open_journal
    first = journal.pin_reader(5)
    assert_equal 5, journal.pin_offset(first)
    assert_equal :released, journal.release(first)
    assert_equal :stale, journal.release(first)
    second = journal.pin_reader(1)
    assert_equal :stale, journal.release(first)
    assert_equal [1, 1], [journal.pins, journal.pin_offset(second)]
    assert_equal :stale, journal.advance(first, 50)
    assert_nil journal.pin_offset(first)
    assert_equal 1, journal.low_pin
    assert_equal :regressed, journal.advance(second, 0)
    assert_equal :unchanged, journal.advance(second, 1)
    assert_equal :wrong_kind, journal.checkpoint(second, 1, 9)
    assert_equal :released, journal.release(second)
    assert_nil journal.low_pin
    assert_equal 0, journal.pins
  end

  def test_checkpoint_assignment_generations
    journal = open_journal
    assert_equal :unassigned, journal.pin_checkpoint(0, 5)
    pin = journal.pin_checkpoint(3, 20)
    assert_equal :wrong_kind, journal.advance(pin, 30)
    assert_equal :unassigned, journal.checkpoint(pin, 0, 30)
    assert_equal :stale_assignment, journal.checkpoint(pin, 2, 99)
    assert_equal :regressed, journal.checkpoint(pin, 3, 19)
    assert_equal :unchanged, journal.checkpoint(pin, 3, 20)
    assert_equal :advanced, journal.checkpoint(pin, 3, 25)
    assert_equal :reassigned, journal.checkpoint(pin, 4, 8)
    assert_equal 8, journal.pin_offset(pin)
    assert_equal :reassigned, journal.checkpoint(pin, 2**64 - 1, 2**64 - 1)
  end

  def test_checkpoints_follow_one_tracker
    journal = open_journal
    assert_equal :unassigned, journal.pin_checkpoint_from(Tracker.new(0, 5))
    assert_equal 0, journal.pins
    pin = journal.pin_checkpoint_from(Tracker.new(3, 20))
    assert_equal [:checkpoint, 20], [pin.kind, journal.pin_offset(pin)]
    assert_equal :unchanged, journal.checkpoint_from(pin, Tracker.new(3, 20))
    assert_equal :advanced, journal.checkpoint_from(pin, Tracker.new(3, 25))
    assert_equal :regressed, journal.checkpoint_from(pin, Tracker.new(3, 24))
    assert_equal :unassigned, journal.checkpoint_from(pin, Tracker.new(0, 99))
    assert_equal :stale_assignment, journal.checkpoint_from(pin, Tracker.new(2, 99))
    assert_equal 25, journal.pin_offset(pin)
    assert_equal :reassigned, journal.checkpoint_from(pin, Tracker.new(4, 8))
    assert_equal :stale_assignment, journal.checkpoint(pin, 3, 30)
    assert_equal 8, journal.pin_offset(pin)
    assert_equal :wrong_kind, journal.checkpoint_from(journal.pin_reader(0), Tracker.new(4, 9))
    assert_equal :stale, open_journal.checkpoint_from(pin, Tracker.new(4, 9))
  end

  def test_tracker_must_have_the_readers
    journal = open_journal
    pin = journal.pin_checkpoint(1, 0)
    [nil, 7, Object.new, Data.define(:generation).new(1), Data.define(:base).new(1)].each do |v|
      assert_raises(TypeError) { journal.pin_checkpoint_from(v) }
      assert_raises(TypeError) { journal.checkpoint_from(pin, v) }
    end
    assert_raises(TypeError) { journal.pin_checkpoint_from(Tracker.new(1.0, 0)) }
    assert_raises(TypeError) { journal.checkpoint_from(pin, Tracker.new(1, nil)) }
    assert_raises(RangeError) { journal.pin_checkpoint_from(Tracker.new(1, -1)) }
    assert_raises(TypeError) { journal.checkpoint_from(Object.new, Tracker.new(1, 5)) }
    assert_equal [1, 0], [journal.pins, journal.pin_offset(pin)]
  end

  def test_pin_capacity
    journal = open_journal
    pins = 32.times.map { journal.pin_reader(it) }
    assert_equal :pins_full, journal.pin_reader(0)
    assert_equal :pins_full, journal.pin_checkpoint(1, 0)
    assert_equal :pins_full, journal.pin_checkpoint_from(Tracker.new(1, 0))
    assert_equal :released, journal.release(pins.last)
    assert_instance_of J::Pin, journal.pin_checkpoint(1, 0)
  end

  def test_strict_inputs
    journal, = journal_with(1)
    pin = journal.pin_reader(0)
    [1.0, Float::NAN, nil, '1', Object.new].each do |v|
      assert_raises(TypeError) { journal.roll(v, 1, 0) }
      assert_raises(TypeError) { journal.roll_decision(1, v, 0) }
      assert_raises(TypeError) { journal.observe_live(1, v) }
      assert_raises(TypeError) { journal.pin_reader(v) }
      assert_raises(TypeError) { journal.advance(pin, v) }
      assert_raises(TypeError) { journal.segments(nil, v) }
      assert_raises(TypeError) { open_journal(base_offset: v) }
    end
    [1.0, '1', Object.new].each do |v|
      assert_raises(TypeError) { open_journal(max_bytes: v) }
      assert_raises(TypeError) { journal.segments(v, 64) }
    end
    [-1, 2**64].each do |v|
      assert_raises(RangeError) { journal.roll(v, 1, 0) }
      assert_raises(RangeError) { journal.pin_checkpoint(1, v) }
      assert_raises(RangeError) { journal.segment(v) }
      assert_raises(RangeError) { open_journal(now: v) }
    end
    assert_raises(RangeError) { open_journal(max_bytes: 0) }
    assert_raises(RangeError) { open_journal(max_records: 0) }
    assert_raises(RangeError) { open_journal(max_age: 0) }
    assert_raises(RangeError) { open_journal(min_dead_per_mille: 1001) }
    assert_equal 2, journal.size
  end

  def test_frozen_journal_refuses_mutation
    journal, = journal_with(1)
    pin = journal.pin_reader(0)
    replacement = journal.propose(1)
    journal.freeze
    assert_raises(FrozenError) { journal.roll(1, 1, 0) }
    assert_raises(FrozenError) { journal.observe_live(1, 0) }
    assert_raises(FrozenError) { journal.pin_reader(0) }
    assert_raises(FrozenError) { journal.pin_checkpoint(1, 0) }
    assert_raises(FrozenError) { journal.advance(pin, 1) }
    assert_raises(FrozenError) { journal.checkpoint(pin, 1, 1) }
    assert_raises(FrozenError) { journal.pin_checkpoint_from(Tracker.new(1, 0)) }
    assert_raises(FrozenError) { journal.checkpoint_from(pin, Tracker.new(1, 1)) }
    assert_raises(FrozenError) { journal.release(pin) }
    assert_raises(FrozenError) { journal.propose(1) }
    assert_raises(FrozenError) { journal.confirm_superseded(replacement) }
    assert_raises(FrozenError) { journal.confirm_rewritten(replacement, 1, 1) }
    assert_raises(FrozenError) { journal.reclaim(replacement) }
    assert_raises(FrozenError) { journal.abort(replacement) }
    assert_equal :keep, journal.roll_decision(1, 1, 0)
    assert_equal 2, journal.segments(nil, 64).size
  end
end
