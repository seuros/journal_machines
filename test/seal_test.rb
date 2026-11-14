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
