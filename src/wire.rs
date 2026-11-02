use num_bigint::BigInt;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

/// Canonical signed decimal JSON string, retaining arbitrary precision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Decimal(pub BigInt);
impl Serialize for Decimal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let digits = value.strip_prefix('-').unwrap_or(&value);
        if digits.is_empty()
            || !digits.bytes().all(|c| c.is_ascii_digit())
            || (digits.len() > 1 && digits.starts_with('0'))
            || value == "-0"
        {
            return Err(D::Error::custom("expected canonical decimal string"));
        }
        Ok(Self(value.parse().map_err(D::Error::custom)?))
    }
}
/// JavaScript-safe JSON number used by the reference presentation API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SafeInteger(pub i64);
impl<'de> Deserialize<'de> for SafeInteger {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Number::deserialize(deserializer)?;
        let number = value
            .as_f64()
            .ok_or_else(|| D::Error::custom("invalid number"))?;
        if number.fract() != 0.0 || number.abs() > 9_007_199_254_740_991.0 {
            return Err(D::Error::custom("expected safe JSON integer"));
        }
        Ok(Self(number as i64))
    }
}
/// A missing optional field is allowed; explicit JSON null is not.
pub fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
