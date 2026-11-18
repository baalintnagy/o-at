//! Bus names chosen by the user.

use std::fmt::{self, Display, Formatter, Write};

/// Maximum length of a bus name in UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 32;

/// Why a text is not a valid bus name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    /// Control characters, such as line breaks, are not allowed.
    InvalidCharacter,
}

impl Display for NameError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Empty => "Enter a bus name",
            Self::TooLong => "Bus names are limited to 32 bytes",
            Self::InvalidCharacter => "Bus names cannot contain control characters",
        };
        formatter.write_str(text)
    }
}

/// A validated bus name. Names are matched case-insensitively, so `Drums` and `drums`
/// are the same bus; the transmitter's spelling is the one displayed.
#[derive(Clone, Debug)]
pub struct BusName {
    display: String,
    key: String,
}

impl BusName {
    /// Validates `text`, ignoring leading and trailing whitespace.
    pub fn new(text: &str) -> Result<Self, NameError> {
        let display = text.trim();
        if display.is_empty() {
            return Err(NameError::Empty);
        }
        if display.len() > MAX_NAME_BYTES {
            return Err(NameError::TooLong);
        }
        if display.chars().any(char::is_control) {
            return Err(NameError::InvalidCharacter);
        }
        Ok(Self {
            display: display.to_owned(),
            key: display.to_lowercase(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.display
    }

    /// Case-folded form used for matching.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Whether `other` names the same bus.
    pub fn matches(&self, other: &str) -> bool {
        other.trim().to_lowercase() == self.key
    }

    /// Portable suffix for operating system object names: the key in hexadecimal.
    pub(crate) fn object_suffix(&self) -> String {
        self.key.bytes().fold(
            String::with_capacity(2 * self.key.len()),
            |mut text, byte| {
                // Writing to a `String` cannot fail.
                let _ = write!(text, "{byte:02x}");
                text
            },
        )
    }
}

impl PartialEq for BusName {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for BusName {}

impl Display for BusName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display)
    }
}

#[cfg(test)]
mod tests {
    use super::{BusName, MAX_NAME_BYTES, NameError};

    #[test]
    fn names_are_trimmed_and_matched_case_insensitively() {
        let name = BusName::new("  Drums Bus ").expect("valid");
        assert_eq!(name.as_str(), "Drums Bus");
        assert_eq!(name, BusName::new("drums bus").expect("valid"));
        assert!(name.matches(" DRUMS BUS"));
        assert_eq!(name.object_suffix(), "6472756d7320627573");
    }

    #[test]
    fn invalid_names_are_rejected() {
        assert_eq!(BusName::new("   ").unwrap_err(), NameError::Empty);
        assert_eq!(
            BusName::new(&"x".repeat(MAX_NAME_BYTES + 1)).unwrap_err(),
            NameError::TooLong
        );
        assert_eq!(
            BusName::new("a\nb").unwrap_err(),
            NameError::InvalidCharacter
        );
        assert!(BusName::new("Bühne 1").is_ok());
    }
}
