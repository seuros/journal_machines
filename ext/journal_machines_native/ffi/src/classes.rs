//! The value classes, resolved once at init, the exact-class reads every
//! value from Ruby goes through, and the clone-then-keep every mutating call
//! whose answer is a Ruby object runs in.
use std::{cell::RefCell, sync::OnceLock};

use magnus::{Error, RClass, RModule, RStruct, Ruby, Value, prelude::*, value::Opaque};

use crate::values::wrong_type;

/// The `Data` classes the binding builds and reads.
#[derive(Clone, Copy)]
pub struct Classes {
    pub policy: RClass,
    pub segment: RClass,
    pub due: RClass,
    pub rolled: RClass,
    pub reclaimed: RClass,
}

/// `Classes` as a static may hold them.
struct Held([Opaque<RClass>; 5]);

static HELD: OnceLock<Held> = OnceLock::new();

/// Resolves the classes `lib/journal_machines.rb` defined before it loaded the
/// extension. Called once, from init; never by name again.
pub fn resolve(ruby: &Ruby, module: RModule) -> Result<(), Error> {
    let names = ["Policy", "Segment", "Due", "Rolled", "Reclaimed"];
    let mut held = [Opaque::from(ruby.class_object()); 5];
    for (slot, name) in held.iter_mut().zip(names) {
        let class: RClass = module.const_get(name)?;
        ruby.gc_register_mark_object(class);
        *slot = Opaque::from(class);
    }
    HELD.set(Held(held)).map_err(|_| {
        Error::new(
            ruby.exception_runtime_error(),
            "journal_machines_native loaded twice",
        )
    })
}

/// The classes resolved at init.
pub fn classes(ruby: &Ruby) -> Result<Classes, Error> {
    let Some(Held(held)) = HELD.get() else {
        return Err(Error::new(
            ruby.exception_runtime_error(),
            "journal_machines_native is not initialised",
        ));
    };
    let [policy, segment, due, rolled, reclaimed] = held.map(|class| ruby.get_inner(class));
    Ok(Classes {
        policy,
        segment,
        due,
        rolled,
        reclaimed,
    })
}

/// Whether `value`'s class is exactly `class`: `instance_of?`, never `is_a?`,
/// and asked natively, so a redefined method cannot answer for it.
fn is_instance_of(value: Value, class: RClass) -> bool {
    let own = value.class();
    own.is_inherited(class) && class.is_inherited(own)
}

/// The fields of a `Data` value of exactly `class`; `TypeError` otherwise,
/// a subclass included. Read with [`RStruct::aref`], which runs no Ruby.
pub fn exact(
    ruby: &Ruby,
    value: Value,
    class: RClass,
    message: &'static str,
) -> Result<RStruct, Error> {
    RStruct::from_value(value)
        .filter(|_| is_instance_of(value, class))
        .ok_or_else(|| wrong_type(ruby, message))
}

/// Runs `act` on a copy of the core in `cell` and keeps the copy only once
/// `act`, which decides and then builds the Ruby answer, has returned. A raise
/// while building leaves the core as it was. Then, right before the store,
/// the receiver is checked again: frozen meanwhile is `FrozenError`, changed
/// meanwhile (a redefined `new` calling back in) is `RuntimeError`, and in
/// both cases the copy is dropped. No borrow is held while `act` runs.
pub fn keep<C: Clone + PartialEq, R>(
    ruby: &Ruby,
    receiver: Value,
    cell: &RefCell<C>,
    act: impl FnOnce(&mut C) -> Result<R, Error>,
) -> Result<R, Error> {
    receiver.check_frozen()?;
    let before = cell.borrow().clone();
    let mut after = before.clone();
    let answer = act(&mut after)?;
    receiver.check_frozen()?;
    let mut held = cell.borrow_mut();
    if *held != before {
        return Err(Error::new(
            ruby.exception_runtime_error(),
            "the journal changed while the answer was built; the call was not applied",
        ));
    }
    *held = after;
    Ok(answer)
}
