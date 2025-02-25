use std::fmt;

use lasso::{Rodeo, Spur};
use std::sync::{LazyLock, RwLock};

/// A opaque identifier for a string which has been [interned].
///
/// Interning strings means they are cheap to copy and compare,
/// at the cost of taking an O(n) hash comparison to intern.
/// Interning also reduces memory usage for programs
/// with many identifiers which are repeated often (i.e. C headers).
///
/// [interned]: https://en.wikipedia.org/wiki/String_interning
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct InternedStr(pub Spur);

impl fmt::Debug for InternedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

pub static STRINGS: LazyLock<RwLock<Rodeo<Spur>>> = LazyLock::new(|| RwLock::new(Rodeo::default()));
static EMPTY_STRING: LazyLock<InternedStr> = LazyLock::new(|| InternedStr::get_or_intern(""));

impl InternedStr {
    /// Return whether `self` is the empty string.
    pub fn is_empty(self) -> bool {
        // Deref the LazyLock to get a reference to EMPTY_STRING.
        self == *EMPTY_STRING
    }

    /// Convert this identifier back into the original `String`, cloning it along the way.
    ///
    /// # Panics
    /// This function will panic if another thread panicked while accessing the global string pool.
    pub fn resolve_and_clone(self) -> String {
        let strings = STRINGS.read().expect("failed to lock String cache for reading");
        let tmp = strings.resolve(&self.0);
        tmp.to_string()
    }

    /// Intern this string into the string pool and return an opaque identifier.
    ///
    /// If `val` is already present, it will not be duplicated (i.e. this method is idempotent).
    ///
    /// # Panics
    /// This function will panic if another thread panicked while accessing the global string pool.
    pub fn get_or_intern<T: AsRef<str> + Into<String>>(val: T) -> InternedStr {
        InternedStr(
            STRINGS
                .write()
                .expect("failed to lock String cache for writing, another thread must have panicked")
                .get_or_intern(val),
        )
    }
}

impl fmt::Display for InternedStr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let strings = crate::saltwater_parser::intern::STRINGS
            .read()
            .expect("failed to lock String cache for reading");
        let tmp = strings.resolve(&self.0);

        write!(f, "{}", tmp)
    }
}

impl Default for InternedStr {
    fn default() -> Self {
        *EMPTY_STRING
    }
}

impl From<&str> for InternedStr {
    fn from(s: &str) -> Self {
        Self::get_or_intern(s)
    }
}

impl From<String> for InternedStr {
    fn from(s: String) -> Self {
        Self::get_or_intern(s)
    }
}

#[cfg(test)]
mod proptest_impl {
    use super::InternedStr;
    use proptest::prelude::*;
    impl Arbitrary for InternedStr {
        type Parameters = String;
        type Strategy = Just<InternedStr>;

        fn arbitrary_with(s: Self::Parameters) -> Self::Strategy {
            Just(s.into())
        }
    }
}
