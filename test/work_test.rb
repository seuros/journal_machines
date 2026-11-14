# frozen_string_literal: true

require_relative 'test_helper'

# The journal never loses work. `roll` and `reclaim` answer Ruby objects, so
# the core decides on a copy, the answer is built, and the copy is kept only
# if the journal is still unfrozen and unchanged. A `new` that raises, calls
# back in, or freezes the journal applies nothing.
class WorkTest < Minitest::Test
  include JournalHelpers

  # Each mutating call that builds a value: the journal it runs on, the call,
  # and the class whose `new` it runs.
  CALLS = {
    roll: [->(_) {}, ->(journal) { journal.roll(1, 1, 0) }, J::Rolled],
    reclaim: [
      ->(journal) { journal.confirm_superseded(journal.propose(1)) },
      ->(journal) { journal.reclaim(journal.segment(1).replacement) },
      J::Reclaimed
    ]
  }.freeze

  # Runs the block with `klass.new` replaced by `body`, the way reopening the
  # class could redefine it; the real `new` is `klass.__real_new`.
  def replacing_new(klass, body)
    klass.singleton_class.alias_method(:__real_new, :new)
    klass.define_singleton_method(:new, &body)
    yield
  ensure
    klass.singleton_class.alias_method(:new, :__real_new)
    klass.singleton_class.remove_method(:__real_new)
  end

  def state(journal) = snapshot(journal, []) + [journal.issued]

  # The journal and a twin given the same history.
  def pair(setup) = Array.new(2) { journal_with(1).first.tap(&setup) }

  def test_a_new_that_raises_applies_nothing
    CALLS.each do |name, (setup, call, klass)|
      journal, twin = pair(setup)
      replacing_new(klass, ->(*) { raise IOError }) { assert_raises(IOError, name.to_s) { call.(journal) } }
      assert_equal state(twin), state(journal), name
      assert_equal call.(twin), call.(journal), name
    end
  end

  def test_a_new_that_calls_back_in_applies_nothing
    CALLS.each do |name, (setup, call, klass)|
      journal, twin = pair(setup)
      reenter = lambda do |*args|
        journal.pin_reader(0)
        klass.__real_new(*args)
      end
      replacing_new(klass, reenter) { assert_raises(RuntimeError, name.to_s) { call.(journal) } }
      twin.pin_reader(0)
      assert_equal state(twin), state(journal), name
      assert_equal call.(twin), call.(journal), name
    end
  end

  def test_a_new_that_freezes_the_journal_applies_nothing
    CALLS.each do |name, (setup, call, klass)|
      journal, twin = pair(setup)
      freeze = lambda do |*args|
        journal.freeze
        klass.__real_new(*args)
      end
      replacing_new(klass, freeze) { assert_raises(FrozenError, name.to_s) { call.(journal) } }
      assert_equal state(twin), state(journal), name
    end
  end
end
