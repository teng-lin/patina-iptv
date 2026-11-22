//! Message identifiers shared by domain rules and the presentation.

/// Mark a message stored as data and translated by the consuming shell.
pub const fn key(source: &'static str) -> &'static str {
    source
}
