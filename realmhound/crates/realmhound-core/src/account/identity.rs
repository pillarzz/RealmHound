use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use uuid::Uuid;

/// Opaque local identifier used as an account profile directory name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountKey(Uuid);

impl AccountKey {
    /// Generate a new opaque account key.
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    /// Return the underlying UUID.
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for AccountKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(formatter)
    }
}

impl FromStr for AccountKey {
    type Err = AccountKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let uuid =
            Uuid::parse_str(value).map_err(|_| AccountKeyError::Invalid(value.to_string()))?;
        if uuid.is_nil() || uuid.hyphenated().to_string() != value {
            return Err(AccountKeyError::Invalid(value.to_string()));
        }
        Ok(Self(uuid))
    }
}

impl Serialize for AccountKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AccountKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Error returned when an account key is not a canonical non-nil UUID.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AccountKeyError {
    /// The value cannot safely identify a profile directory.
    #[error("invalid account key `{0}`")]
    Invalid(String),
}

/// Normalized server-provided account identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(String);

impl AccountId {
    /// Normalize a server account ID while preserving case-sensitive identity.
    pub fn new(value: impl AsRef<str>) -> Result<Self, AccountIdError> {
        let value = value.as_ref().trim();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(AccountIdError::Invalid);
        }
        Ok(Self(value.to_string()))
    }

    /// Return the normalized server account ID.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for AccountId {
    type Err = AccountIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for AccountId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AccountId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Error returned when a server account ID is empty or contains controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AccountIdError {
    /// The account ID is not a usable opaque identity.
    #[error("account ID cannot be empty or contain control characters")]
    Invalid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_key_round_trips_canonical_uuid() {
        let key = AccountKey::generate();

        assert_eq!(key.to_string().parse(), Ok(key));
    }

    #[test]
    fn account_key_rejects_uppercase_uuid() {
        let value = AccountKey::generate().to_string().to_uppercase();

        assert!(value.parse::<AccountKey>().is_err());
    }

    #[test]
    fn account_key_rejects_path_traversal() {
        assert!("../profile".parse::<AccountKey>().is_err());
    }

    #[test]
    fn account_id_trims_surrounding_whitespace() {
        let account_id = AccountId::new("  ABC123  ").unwrap();

        assert_eq!(account_id.as_str(), "ABC123");
    }

    #[test]
    fn account_id_preserves_case() {
        let upper = AccountId::new("ABC123").unwrap();
        let lower = AccountId::new("abc123").unwrap();

        assert_ne!(upper, lower);
    }

    #[test]
    fn account_id_rejects_control_characters() {
        assert!(AccountId::new("abc\n123").is_err());
    }
}
