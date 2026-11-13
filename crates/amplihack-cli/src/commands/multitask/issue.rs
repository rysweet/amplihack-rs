//! One optional-issue parser for model, configuration and persistence boundaries.
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct IssueNumber(NonZeroU64);

impl IssueNumber {
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl<'de> Deserialize<'de> for IssueNumber {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        parse_optional(&Value::deserialize(d)?)
            .and_then(|v| v.ok_or_else(|| anyhow::anyhow!("expected a positive issue number")))
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueRef {
    pub repository: String,
    pub number: IssueNumber,
}

pub fn parse_optional(value: &Value) -> Result<Option<IssueNumber>> {
    let number = match value {
        Value::Null => return Ok(None),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty()
                || [
                    "tbd",
                    "tbc",
                    "todo",
                    "n/a",
                    "none",
                    "null",
                    "unknown",
                    "unassigned",
                    "pending",
                ]
                .iter()
                .any(|p| s.eq_ignore_ascii_case(p))
            {
                return Ok(None);
            }
            ensure!(
                s.bytes().all(|b| b.is_ascii_digit()),
                "issue must be a positive decimal integer or absent"
            );
            s.parse::<u64>()
                .map_err(|_| anyhow::anyhow!("issue exceeds u64::MAX"))?
        }
        Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("issue must be a positive integer"))?,
        _ => bail!("issue must be a positive integer, decimal string or absent"),
    };
    Ok(Some(IssueNumber(NonZeroU64::new(number).ok_or_else(
        || anyhow::anyhow!("issue must be greater than zero"),
    )?)))
}

pub fn deserialize_value<'de, D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    let value = Value::deserialize(d)?;
    parse_optional(&value)
        .map(|v| v.map_or(Value::Null, |n| Value::from(n.get())))
        .map_err(serde::de::Error::custom)
}

pub fn deserialize_legacy<'de, D: Deserializer<'de>>(d: D) -> Result<i128, D::Error> {
    let value = Value::deserialize(d)?;
    parse_optional(&value)
        .map(|v| v.map_or(0, |n| i128::from(n.get())))
        .map_err(serde::de::Error::custom)
}

pub fn is_zero(n: &i128) -> bool {
    *n == 0
}
