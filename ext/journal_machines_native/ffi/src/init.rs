//! Private native entry point.
use crate::{Journal, PINS, PinHandle, ReplacementHandle, SEGMENTS};
use magnus::{Error, Ruby, function, method, prelude::*};

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("JournalMachines")?;
    module.const_set("SEGMENT_CAPACITY", SEGMENTS)?;
    module.const_set("PIN_CAPACITY", PINS)?;
    let pin = module.define_class("Pin", ruby.class_object())?;
    pin.undef_default_alloc_func();
    pin.define_method("kind", method!(PinHandle::kind, 0))?;
    let replacement = module.define_class("Replacement", ruby.class_object())?;
    replacement.undef_default_alloc_func();
    replacement.define_method("segment", method!(ReplacementHandle::segment, 0))?;
    replacement.define_method("attempt", method!(ReplacementHandle::attempt, 0))?;
    let class = module.define_class("Journal", ruby.class_object())?;
    class.undef_default_alloc_func();
    class.define_singleton_method("__open", function!(Journal::open, 6))?;
    class.define_method("size", method!(Journal::size, 0))?;
    class.define_method("roll_decision", method!(Journal::roll_decision, 3))?;
    class.define_method("__roll", method!(Journal::roll, 3))?;
    class.define_method("observe_live", method!(Journal::observe_live, 2))?;
    class.define_method("__active", method!(Journal::active, 0))?;
    class.define_method("__segment", method!(Journal::segment, 1))?;
    class.define_method("__covering", method!(Journal::covering, 1))?;
    class.define_method("__scan", method!(Journal::scan, 3))?;
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
    class.define_method("__reclaim", method!(Journal::reclaim, 1))?;
    class.define_method("abort", method!(Journal::abort, 1))?;
    Ok(())
}
