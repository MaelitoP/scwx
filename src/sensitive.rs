//! A value that must never appear in logs or error chains.

use std::fmt;

/// Debug prints a placeholder instead of the content.
#[derive(Clone)]
pub(crate) struct Sensitive(String);

impl Sensitive {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Sensitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_value() {
        let secret = Sensitive::new("hunter2".to_owned());
        assert_eq!(format!("{secret:?}"), "[redacted]");
        assert_eq!(secret.expose(), "hunter2");
    }
}
