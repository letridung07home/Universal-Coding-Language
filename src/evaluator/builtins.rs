//! The built-in function prelude.
//!
//! Each variant is a callable that the evaluator dispatches to directly;
//! [`BuiltinFunction::name`] is the single source of truth for the
//! source-level name users write.

/// A built-in callable supplied by the UCL prelude.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinFunction {
    /// Returns the number of Unicode scalar values in a string.
    Len,
    /// Returns the result-echo text form of any value.
    Str,
    /// Returns the name of a value's type.
    Type,
    /// Converts a string to upper case.
    Upper,
    /// Converts a string to lower case.
    Lower,
    /// Reports whether one string contains another as a substring.
    Contains,
}

impl BuiltinFunction {
    /// Returns the source-level name used to look up this built-in.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Len => "len",
            Self::Str => "str",
            Self::Type => "type",
            Self::Upper => "upper",
            Self::Lower => "lower",
            Self::Contains => "contains",
        }
    }

    /// Iterates over every built-in, in prelude registration order.
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        [
            Self::Len,
            Self::Str,
            Self::Type,
            Self::Upper,
            Self::Lower,
            Self::Contains,
        ]
        .into_iter()
    }
}
