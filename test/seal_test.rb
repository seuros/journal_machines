# frozen_string_literal: true

require_relative 'test_helper'

# The values are sealed, and sealing stops accidents, not reflection: the
# extension is the authority, and the one value it reads back, the Policy a
# journal opens with, must be exactly a Policy and valid by the core.
class SealTest < Minitest::Test
  include JournalHelpers

  VALUES = [J::Policy, J::Segment, J::Due, J::Rolled, J::Reclaimed].freeze

  # A named subclass raises too, but Ruby binds the constant before
  # `inherited` runs: the class leaks, and can be reopened to skip the check.
  def leak(value)
    name = :"Leak#{value.name.split('::').last}"
    return self.class.const_get(name) if self.class.const_defined?(name, false)

    assert_raises(TypeError) { self.class.class_eval("class #{name} < #{value}; end", __FILE__, __LINE__) }
    self.class.const_get(name).tap do |leaked|
      leaked.class_eval { def initialize(**fields) = Data.instance_method(:initialize).bind_call(self, **fields) }
    end
  end

  def test_values_cannot_be_subclassed_or_allocated
    VALUES.each do |value|
      assert_raises(TypeError) { Class.new(value) }
      assert_operator leak(value), :<, value
      assert_raises(NoMethodError) { value.allocate }
    end
  end

  # A forged policy never reaches the core: not a leaked subclass, not one
  # whose fields `Marshal.load` rewrote.
  def test_a_forged_policy_is_refused
    forged = leak(J::Policy).new(max_bytes: 0, max_records: nil, max_age: nil, min_dead_per_mille: 5000)
    assert_raises(TypeError) { J::Journal.new(forged, 0, 0) }
    assert_raises(TypeError) { J::Journal.with_scope([nil, nil, nil, 500], 0, 0, 1) }

    ratio = Marshal.load(Marshal.dump(J::Policy.new(nil, nil, nil, 500)).sub("i\x02\xF4\x01".b, "i\x02\xD0\x07".b))
    assert_equal 2000, ratio.min_dead_per_mille
    zero = Marshal.load(Marshal.dump(J::Policy.new(1, nil, nil, 500)).sub("i\x06", "i\x00"))
    assert_equal 0, zero.max_bytes
    [ratio, zero].each do |policy|
      assert_raises(RangeError) { J::Journal.new(policy, 0, 0) }
      assert_raises(RangeError) { J::Journal.resumed(policy, 0, 0, 0, 0) }
    end
  end

  # A singleton class opened on a genuine Policy does not change what it is,
  # a clone of one included.
  def test_an_opened_singleton_class_is_still_the_gems_value
    policy = J::Policy.new(100, nil, nil, 250).tap(&:singleton_class)
    assert_equal policy, J::Journal.new(policy, 0, 0).policy
    assert_equal policy, J::Journal.resumed(policy.clone, 0, 0, 1, 0).policy
  end

  # The extension reads a Policy's fields from the struct, never through its
  # readers: a redefined reader is ignored.
  def test_a_redefined_reader_is_ignored
    policy = J::Policy.new(100, nil, nil, 250)
    readers = %i[max_bytes min_dead_per_mille].to_h { [it, J::Policy.instance_method(it)] }
    begin
      J::Policy.define_method(:max_bytes) { 0 }
      J::Policy.define_method(:min_dead_per_mille) { raise 'reader ran' }
      journal = J::Journal.new(policy, 0, 0)
    ensure
      readers.each { |name, reader| J::Policy.define_method(name, reader) }
    end
    assert_equal policy, journal.policy
  end

  # The arrays the native hooks read are exactly Arrays: a subclass is a
  # TypeError like any other subclass.
  def test_an_array_subclass_is_refused
    list = Class.new(Array)
    head = [1, 0, 0, 0]
    counts = [0, 0, 0, 0]
    assert_raises(TypeError) { J::Journal.__send__(:__check_segment, list[*head], counts, :active, false, nil) }
    assert_raises(TypeError) { J::Journal.__send__(:__check_segment, head, list[*counts], :active, false, nil) }
    assert_raises(TypeError) { J::Journal.__send__(:__check_segment, head, [0, 0, 0], :active, false, nil) }
    policy = J::Policy.new(nil, nil, nil, 500)
    assert_raises(TypeError) { J::Journal.__send__(:__open, 0, 0, list[0, 0], policy) }
    assert_raises(TypeError) { J::Journal.__send__(:__open, 0, 0, [0], policy) }
    assert_equal 1, J::Journal.__send__(:__open, 0, 0, [0, 0], policy).issued
  end

  # A handle is read natively by its exact class; a singleton class opened
  # on a genuine one changes nothing, and nothing else stands in for it.
  def test_handles_are_read_by_exact_class
    journal, = journal_with(1)
    pin = journal.pin_reader(3).tap(&:singleton_class)
    assert_equal 3, journal.pin_offset(pin)
    assert_equal :moved, journal.advance(pin, 4)
    replacement = journal.propose(1)
    [replacement, Object.new, 1].each { |v| assert_raises(TypeError) { journal.advance(v, 5) } }
    [pin, Object.new].each { |v| assert_raises(TypeError) { journal.abort(v) } }
    assert_raises(TypeError) { journal.pin_offset(replacement) }
    assert_equal :aborted, journal.abort(replacement.tap(&:singleton_class))
  end

  # The classes the extension resolved at init are pinned: after a
  # compaction that moves what it can, every path that builds or reads a
  # value still finds them.
  def test_resolved_classes_survive_compaction
    GC.verify_compaction_references(expand_heap: true, toward: :empty) if GC.respond_to?(:verify_compaction_references)
    GC.compact if GC.respond_to?(:compact)
    journal = open_journal(max_bytes: 10)
    assert_equal J::Due.new(:size), journal.roll_decision(20, 1, 0)
    rolled = journal.roll(20, 1, 0)
    assert_instance_of J::Rolled, rolled
    journal.confirm_superseded(journal.propose(rolled.sealed))
    assert_instance_of J::Reclaimed, journal.reclaim(journal.segment(rolled.sealed).replacement)
    assert_instance_of J::Segment, journal.active
    assert_equal 1, journal.segments(nil, 64).size
    assert_equal journal.policy, J::Journal.new(journal.policy, 0, 0).policy
  end
end
