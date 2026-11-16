# frozen_string_literal: true

require_relative 'test_helper'
require 'open3'
require 'rbconfig'

# The classes of the values the extension wraps (Journal, Pin, Replacement)
# are fixed when it loads. Nothing a caller does later, redefining a constant
# or `Object.const_get`, reaches a handle being built. Each case runs in a
# fresh process: the class is fixed once per process, at load.
class ClassLookupTest < Minitest::Test
  LIB = File.expand_path('../lib', __dir__)

  PRELUDE = <<~RUBY
    require 'journal_machines'
    J = JournalMachines
    journal = J::Journal.new(J::Policy.new(nil, nil, nil, 500), 0, 0)
    sealed = journal.roll(100, 10, 0).sealed
  RUBY

  # Runs `body` after PRELUDE in a new Ruby; answers its last line of output.
  def isolated(body)
    out, err, status = Open3.capture3(RbConfig.ruby, '-I', LIB, '-e', PRELUDE + body)
    assert status.success?, "exited #{status.exitstatus}: #{err}"
    out.lines.last&.chomp
  end

  # Bug: a Pin constant redefined before the first pin made the handle an
  # instance of the new class, so #release raised TypeError and the pin was
  # stranded.
  def test_a_redefined_pin_constant_does_not_change_the_handle_class
    assert_equal 'true released 0', isolated(<<~RUBY)
      original = J::Pin
      J.send(:remove_const, :Pin)
      J.const_set(:Pin, Class.new)
      handle = journal.pin_reader(0)
      puts [handle.instance_of?(original), journal.release(handle), journal.pins].join(' ')
    RUBY
  end

  # Bug: the same for a Replacement and #abort.
  def test_a_redefined_replacement_constant_does_not_change_the_handle_class
    assert_equal 'true aborted', isolated(<<~RUBY)
      original = J::Replacement
      J.send(:remove_const, :Replacement)
      J.const_set(:Replacement, Class.new)
      handle = journal.propose(sealed)
      puts [handle.instance_of?(original), journal.abort(handle)].join(' ')
    RUBY
  end

  # A Journal opened after its constant was redefined is still a Journal.
  def test_a_redefined_journal_constant_does_not_change_the_journal_class
    assert_equal 'true', isolated(<<~RUBY)
      original = J::Journal
      policy = J::Policy.new(nil, nil, nil, 500)
      J.send(:remove_const, :Journal)
      J.const_set(:Journal, Class.new)
      puts original.new(policy, 0, 0).instance_of?(original)
    RUBY
  end

  # Bug: building the first handle ran `Object.const_get`, so a redefined one
  # could freeze the journal after the core mutated it, and the mutation was
  # kept. No Ruby runs between the checks and the core call now.
  def test_building_a_handle_runs_no_const_get
    assert_equal '0 false 1 Pin Replacement', isolated(<<~RUBY)
      hits = 0
      Object.define_singleton_method(:const_get) do |*args|
        hits += 1
        journal.freeze
        super(*args)
      end
      pin = journal.pin_reader(0)
      replacement = journal.propose(sealed)
      puts [hits, journal.frozen?, journal.pins, pin.class.name.split('::').last,
            replacement.class.name.split('::').last].join(' ')
    RUBY
  end

  # Bug: a redefined `Object.const_get` that raised hit an `unwrap` in the
  # extension: an uncatchable fatal, and the process exited.
  def test_a_raising_const_get_does_not_kill_the_process
    assert_equal 'released aborted', isolated(<<~RUBY)
      Object.define_singleton_method(:const_get) { |*| raise IOError, 'lookup ran after the mutation' }
      pin = journal.pin_reader(0)
      replacement = journal.propose(sealed)
      puts [journal.release(pin), journal.abort(replacement)].join(' ')
    RUBY
  end
end
