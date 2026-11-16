//! The value classes, resolved once at init and pinned. The exact-class
//! reads and clone-then-keep are `matryona_machines::ruby`'s.
use magnus::{Error, RClass, RModule, Ruby};
use matryona_machines::ruby::Class;

use crate::{PinHandle, ReplacementHandle};

/// The `Data` classes the binding builds and reads.
pub struct Classes {
    pub policy: Class,
    pub segment: Class,
    pub due: Class,
    pub rolled: Class,
    pub reclaimed: Class,
    pub pin: Class,
    pub replacement: Class,
}

matryona_machines::registry! {
    static CLASSES: Classes = "journal_machines";
    /// The classes resolved at init.
    pub fn classes;
}

/// Resolves the classes `lib/journal_machines.rb` defined before it loaded the
/// extension and pins the two handle classes init defined, fixing magnus's
/// class for each handle type to them: no handle conversion runs
/// `Object.const_get` later. Called once, from init; never by name again.
pub fn resolve(
    ruby: &Ruby,
    module: RModule,
    pin: RClass,
    replacement: RClass,
) -> Result<(), Error> {
    let class = |name| Class::resolve(ruby, module, name);
    let classes = Classes {
        policy: class("Policy")?,
        segment: class("Segment")?,
        due: class("Due")?,
        rolled: class("Rolled")?,
        reclaimed: class("Reclaimed")?,
        pin: Class::pin_wrapped::<PinHandle>(ruby, pin)?,
        replacement: Class::pin_wrapped::<ReplacementHandle>(ruby, replacement)?,
    };
    CLASSES.set(ruby, classes).map(drop)
}
