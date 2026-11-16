# frozen_string_literal: true

require_relative 'test_helper'

# Values are valid or do not exist; scans resume only from issued ids; a call
# that raises has changed nothing.
class ValuesTest < Minitest::Test
  include JournalHelpers

  def test_a_due_names_one_of_the_three_reasons
    assert_equal J::Due.new(:size), J::Due.new(reason: :size)
    %i[size records age].each { |reason| assert_equal reason, J::Due.new(reason).reason }
    assert_raises(RangeError) { J::Due.new(:nope) }
    ['size', 1, nil].each { |bad| assert_raises(TypeError) { J::Due.new(bad) } }
    assert_raises(ArgumentError) { J::Due.new }
  end

  def test_a_rolled_is_one_a_journal_records
    journal = open_journal(scope: 3)
    rolled = journal.roll(100, 10, 0)
    assert_equal rolled, J::Rolled.new(*rolled.deconstruct)
    assert_equal J::Rolled.new(1, 2, 10), J::Rolled.new(sealed: 1, opened: 2, next_offset: 10)
    [[0, 2, 10], [2, 2, 10], [3, 2, 10], [1, (1 << 48) | 2, 10], [1, 2, 0], [-1, 2, 10]].each do |fields|
      assert_raises(RangeError, fields.inspect) { J::Rolled.new(*fields) }
    end
    assert_raises(TypeError) { J::Rolled.new(1, 2.0, 10) }
    assert_raises(TypeError) { J::Rolled.new(nil, 2, 10) }
  end

  def test_a_reclaimed_is_one_a_journal_answers
    journal, = journal_with(1)
    handle = journal.propose(1)
    journal.confirm_rewritten(handle, 40, 4)
    reclaimed = journal.reclaim(handle)
    assert_equal reclaimed, J::Reclaimed.new(*reclaimed.deconstruct)
    assert_equal J::Reclaimed.new(1, 0, 9, nil), J::Reclaimed.new(segment: 1, revision: 0, bytes: 9, replaced_by: nil)
    [[0, 0, 9, nil], [3, 2, 9, nil], [1, 0, 9, 1], [1, 5, 9, 5], [1, 5, 9, 4]].each do |fields|
      assert_raises(RangeError, fields.inspect) { J::Reclaimed.new(*fields) }
    end
    assert_raises(TypeError) { J::Reclaimed.new(1, 0, '9', nil) }
    assert_raises(TypeError) { J::Reclaimed.new(1, 0, 9, 5.0) }
  end

  def test_a_segment_is_one_a_journal_answers
    journal, = journal_with(2)
    handle = journal.propose(1)
    proposed, plain, active = journal.segments(nil, 3)
    [proposed, plain, active].each { |segment| assert_equal segment, J::Segment.new(*segment.deconstruct) }
    assert_equal active, J::Segment.new(**active.to_h)
    bad = [
      active.to_h.merge(records: 1), active.to_h.merge(retention: :pinned), active.to_h.merge(replacement: handle),
      plain.to_h.merge(stored_records: 11), plain.to_h.merge(live_records: 11), plain.to_h.merge(id: 0),
      plain.to_h.merge(state: :proposed), proposed.to_h.merge(replacement: nil), proposed.to_h.merge(state: :durable),
      plain.to_h.merge(state: :nope), plain.to_h.merge(retention: :nope)
    ]
    bad.each { |fields| assert_raises(RangeError, fields.inspect) { J::Segment.new(**fields) } }
  end

  # Bug: a segment at revision 0 storing fewer records than its range was
  # accepted, though only a rewrite, which assigns a revision, stores fewer.
  def test_a_segment_never_rewritten_stores_every_record
    plain = journal_with(1).first.segment(1)
    assert_raises(RangeError) { J::Segment.new(1, 0, 0, 1, 0, 0, 0, 0, :sealed, :unpinned, nil) }
    assert_raises(RangeError) { J::Segment.new(**plain.to_h, stored_records: 9, live_records: 9) }
    rewritten = J::Segment.new(**plain.to_h, revision: 5, stored_records: 4, live_records: 4)
    assert_equal [5, 4], [rewritten.revision, rewritten.stored_records]
  end

  def test_a_segment_raises_type_error_for_a_wrong_type
    active = open_journal.active
    changes = [{ retention: nil }, { retention: true }, { state: 'active' }, { replacement: 5 }, { bytes: 1.0 },
               { opened_at: nil }]
    changes.each { |change| assert_raises(TypeError, change.inspect) { J::Segment.new(**active.to_h, **change) } }
    assert_raises(RangeError) { active.with(records: 1) }
  end

  def test_retention_is_exactly_a_retention_symbol
    active = open_journal.active
    assert_equal :unpinned, active.retention
    [nil, false, true, 'pinned'].each do |flag|
      assert_raises(TypeError, flag.inspect) { J::Segment.new(**active.to_h, retention: flag) }
    end
    assert_raises(RangeError) { J::Segment.new(**active.to_h, retention: :nope) }
  end

  def test_a_cursor_is_an_identity_this_journal_issued
    journal, = journal_with(3)
    assert_equal [2, 3, 4], journal.segments(1, 10).map(&:id)
    [0, journal.issued + 1, (1 << 48) | 2].each do |never|
      assert_empty journal.segments(never, 10), never.to_s
      assert_empty journal.compaction_candidates(never, 10)
      assert_empty journal.reclaimable(never, 10)
    end
    handle = journal.propose(1)
    journal.confirm_superseded(handle)
    journal.reclaim(handle)
    assert_nil journal.segment(1)
    assert_equal [2, 3, 4], journal.segments(1, 10).map(&:id)
    assert_raises(TypeError) { journal.segments('1', 10) }
    assert_raises(RangeError) { journal.segments(-1, 10) }
  end

  # Regression: two reads of one proposed segment answered handles that were
  # not ==, so neither were the segments holding them.
  def test_handles_are_values
    journal, = journal_with(2)
    handle = journal.propose(1)
    read = journal.segment(1).replacement
    refute_same handle, read
    assert_equal [handle, true, handle.hash], [read, read.eql?(handle), read.hash]
    assert_equal journal.segment(1), journal.segment(1)
    assert_equal 1, { handle => 1, read => 2 }.size
    pin = journal.pin_reader(0)
    [journal.propose(2), pin, journal.pin_reader(0), 1, nil].each { refute_equal handle, it }
    refute_equal pin, journal.pin_reader(0)
  end

  # The binding reads everything that can raise before the journal is asked.
  def test_a_call_that_raises_changed_nothing
    journal = open_journal
    before = [journal.pins, journal.issued]
    exploding = Object.new
    def exploding.generation = 1
    def exploding.base = raise(IOError, 'tracker went away')
    assert_raises(IOError) { journal.pin_checkpoint_from(exploding) }
    assert_raises(TypeError) { journal.pin_reader(1.5) }
    assert_raises(TypeError) { journal.roll(100, nil, 0) }
    assert_equal before, [journal.pins, journal.issued]
    assert_equal 1, journal.size
    pin = journal.pin_reader(0)
    assert_raises(IOError) { journal.checkpoint_from(pin, exploding) }
    assert_equal 0, journal.pin_offset(pin)
  end

  # A tracker is a sibling gem's value, read natively by its public readers:
  # a missing one is named before any reader runs.
  def test_a_tracker_is_read_natively_by_its_public_readers
    journal = open_journal
    calls = []
    partial = Object.new
    partial.define_singleton_method(:generation) { calls << :generation && 0 }
    error = assert_raises(TypeError) { journal.pin_checkpoint_from(partial) }
    assert_equal 'expected an offset tracker responding to #generation and #base, Object lacks #base', error.message
    assert_empty calls
    error = assert_raises(TypeError) { journal.pin_checkpoint_from(5) }
    assert_equal 'expected an offset tracker responding to #generation and #base, Integer lacks #generation',
                 error.message

    pin = journal.pin_checkpoint_from(Tracker.new(1, 1))
    assert_equal 1, journal.pin_offset(pin)
    assert_equal :advanced, journal.checkpoint_from(pin, Tracker.new(1, 5))
    assert_equal 5, journal.pin_offset(pin)
  end

  # `respond_to?` is asked, so one that lies is believed: denying a reader the
  # tracker has refuses it; claiming one it lacks runs into NoMethodError.
  def test_a_lying_respond_to_is_believed
    journal = open_journal
    denying = Object.new
    def denying.generation = 1
    def denying.base = 1
    def denying.respond_to?(*) = false
    assert_raises(TypeError) { journal.pin_checkpoint_from(denying) }
    claiming = Object.new
    def claiming.generation = 1
    def claiming.respond_to?(*) = true
    assert_raises(NoMethodError) { journal.pin_checkpoint_from(claiming) }
    assert_equal 0, journal.pins
  end

  def test_a_private_reader_is_missing
    journal = open_journal
    tracker = Object.new
    def tracker.generation = 0
    tracker.define_singleton_method(:base) { 1 }
    tracker.singleton_class.send(:private, :base)
    error = assert_raises(TypeError) { journal.pin_checkpoint_from(tracker) }
    assert_includes error.message, 'lacks #base'
  end
end
