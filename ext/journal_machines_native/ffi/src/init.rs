//! Private native entry point.
use crate::{Journal, PINS, PinHandle, ReplacementHandle, SEGMENTS, values};
use magnus::{
    Error, RClass, Ruby, function, method,
    prelude::*,
    typed_data::{Hash, IsEql},
};

/// `==`, `eql?` and `hash` by the identity a handle carries.
macro_rules! value_object {
    ($class:expr, $handle:ty) => {{
        let class: RClass = $class;
        class.define_method("==", method!(<$handle as IsEql>::is_eql, 1))?;
        class.define_method("eql?", method!(<$handle as IsEql>::is_eql, 1))?;
        class.define_method("hash", method!(<$handle as Hash>::hash, 0))?;
    }};
}

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("JournalMachines")?;
    module.const_set("SEGMENT_CAPACITY", SEGMENTS)?;
    module.const_set("PIN_CAPACITY", PINS)?;
    crate::classes::resolve(ruby, module)?;
    let pin = module.define_class("Pin", ruby.class_object())?;
    pin.undef_default_alloc_func();
    pin.define_method("kind", method!(PinHandle::kind, 0))?;
    pin.define_method("generation", method!(PinHandle::generation, 0))?;
    pin.define_method("scope", method!(PinHandle::scope, 0))?;
    value_object!(pin, PinHandle);
    let replacement = module.define_class("Replacement", ruby.class_object())?;
    replacement.undef_default_alloc_func();
    replacement.define_method("segment", method!(ReplacementHandle::segment, 0))?;
    replacement.define_method("attempt", method!(ReplacementHandle::attempt, 0))?;
    replacement.define_method("scope", method!(ReplacementHandle::scope, 0))?;
    value_object!(replacement, ReplacementHandle);
    let class = module.define_class("Journal", ruby.class_object())?;
    class.undef_default_alloc_func();
    class.define_singleton_method("__open", function!(Journal::open, 4))?;
    class.define_method("scope", method!(Journal::scope, 0))?;
    class.define_method("issued", method!(Journal::issued, 0))?;
    class.define_singleton_method("__check_policy", function!(Journal::check_policy, 4))?;
    class.define_singleton_method("__check_due", function!(values::check_due, 1))?;
    class.define_singleton_method("__check_rolled", function!(values::check_rolled, 3))?;
    class.define_singleton_method("__check_reclaimed", function!(values::check_reclaimed, 4))?;
    class.define_singleton_method("__check_segment", function!(values::check_segment, 5))?;
    class.define_method("__policy", method!(Journal::policy, 0))?;
    class.define_method("size", method!(Journal::size, 0))?;
    class.define_method("empty?", method!(Journal::is_empty, 0))?;
    class.define_method("roll_decision", method!(Journal::roll_decision, 3))?;
    class.define_method("roll", method!(Journal::roll, 3))?;
    class.define_method("observe_live", method!(Journal::observe_live, 2))?;
    class.define_method("active", method!(Journal::active, 0))?;
    class.define_method("segment", method!(Journal::segment, 1))?;
    class.define_method("covering", method!(Journal::covering, 1))?;
    class.define_method("segments", method!(Journal::segments, 2))?;
    class.define_method(
        "compaction_candidates",
        method!(Journal::compaction_candidates, 2),
    )?;
    class.define_method("reclaimable", method!(Journal::reclaimable, 2))?;
    class.define_method("pin_reader", method!(Journal::pin_reader, 1))?;
    class.define_method("pin_checkpoint", method!(Journal::pin_checkpoint, 2))?;
    class.define_method("advance", method!(Journal::advance, 2))?;
    class.define_method("checkpoint", method!(Journal::checkpoint, 3))?;
    class.define_method("release", method!(Journal::release, 1))?;
    class.define_method("pin_offset", method!(Journal::pin_offset, 1))?;
    class.define_method("low_pin", method!(Journal::low_pin, 0))?;
    class.define_method("pins", method!(Journal::pins, 0))?;
    class.define_method("propose", method!(Journal::propose, 1))?;
    class.define_method("confirm_rewritten", method!(Journal::confirm_rewritten, 3))?;
    class.define_method(
        "confirm_superseded",
        method!(Journal::confirm_superseded, 1),
    )?;
    class.define_method("reclaim", method!(Journal::reclaim, 1))?;
    class.define_method("abort", method!(Journal::abort, 1))?;
    Ok(())
}
