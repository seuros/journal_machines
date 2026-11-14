//! The two handle classes: values that compare and hash by the identity
//! they carry, so two answers naming the same pin or attempt are `==`.
use journal_machines::{Pin, PinKind, Replacement};
use magnus::{Ruby, Symbol};

#[derive(PartialEq, Eq, Hash)]
#[magnus::wrap(class = "JournalMachines::Pin", free_immediately, size)]
pub struct PinHandle {
    pub pin: Pin,
}

impl PinHandle {
    pub fn kind(ruby: &Ruby, this: &Self) -> Symbol {
        ruby.to_symbol(match this.pin.kind() {
            PinKind::Reader => "reader",
            PinKind::Checkpoint => "checkpoint",
        })
    }

    pub const fn generation(&self) -> u64 {
        self.pin.generation()
    }

    pub const fn scope(&self) -> u16 {
        self.pin.scope()
    }
}

#[derive(PartialEq, Eq, Hash)]
#[magnus::wrap(class = "JournalMachines::Replacement", free_immediately, size)]
pub struct ReplacementHandle {
    pub replacement: Replacement,
}

impl ReplacementHandle {
    pub const fn segment(&self) -> u64 {
        self.replacement.segment().raw()
    }

    pub const fn attempt(&self) -> u64 {
        self.replacement.attempt()
    }

    pub const fn scope(&self) -> u16 {
        self.replacement.scope()
    }
}
