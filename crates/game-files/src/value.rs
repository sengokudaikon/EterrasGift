//! A decoded object: what its type tree said, field by field. Accessors fail with the field's name rather than
//! defaulting, so a game update that moves or retypes a field is reported, never read as zero.

use crate::error::{Error, Result};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Pair(Box<(Value, Value)>),
    Object(Vec<(Arc<str>, Value)>),
}

impl Value {
    fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "a bool",
            Self::Int(_) | Self::UInt(_) => "an integer",
            Self::Float(_) => "a float",
            Self::String(_) => "a string",
            Self::Bytes(_) => "bytes",
            Self::Array(_) => "an array",
            Self::Pair(_) => "a pair",
            Self::Object(_) => "an object",
        }
    }

    fn expected(&self, what: &str) -> Error {
        Error::data(format!("expected {what}, found {}", self.kind()))
    }

    pub fn get(&self, name: &str) -> Result<&Value> {
        match self {
            Self::Object(fields) => fields
                .iter()
                .find(|(field, _)| &**field == name)
                .map(|(_, value)| value)
                .ok_or_else(|| Error::data(format!("no field {name}"))),
            other => Err(other.expected("an object").context(name)),
        }
    }

    /// `get`, with the field's name on any error the accessor gives.
    pub fn field<'a, T>(&'a self, name: &str, read: impl FnOnce(&'a Value) -> Result<T>) -> Result<T> {
        self.get(name).and_then(read).map_err(|e| e.context(name))
    }

    pub fn as_i64(&self) -> Result<i64> {
        match self {
            Self::Int(v) => Ok(*v),
            Self::UInt(v) => i64::try_from(*v).map_err(|_| Error::data(format!("{v} is out of range"))),
            Self::Bool(v) => Ok((*v).into()),
            other => Err(other.expected("an integer")),
        }
    }

    /// An integer converted to `T`, refused if it does not fit.
    pub fn int<T: TryFrom<i64>>(&self) -> Result<T> {
        let value = self.as_i64()?;
        T::try_from(value).map_err(|_| Error::data(format!("{value} is out of range for {}", std::any::type_name::<T>())))
    }

    pub fn as_bool(&self) -> Result<bool> {
        match self {
            Self::Bool(v) => Ok(*v),
            Self::Int(0) | Self::UInt(0) => Ok(false),
            Self::Int(1) | Self::UInt(1) => Ok(true),
            other => Err(other.expected("a bool")),
        }
    }

    pub fn as_f32(&self) -> Result<f32> {
        match self {
            Self::Float(v) => Ok(*v as f32),
            other => Err(other.expected("a float")),
        }
    }

    pub fn as_str(&self) -> Result<&str> {
        match self {
            Self::String(v) => Ok(v),
            other => Err(other.expected("a string")),
        }
    }

    /// [`Value::as_str`], owned.
    pub fn as_string(&self) -> Result<String> {
        self.as_str().map(str::to_owned)
    }

    pub fn as_array(&self) -> Result<&[Value]> {
        match self {
            Self::Array(items) => Ok(items),
            // An empty byte-typed vector decodes as bytes; it is still an empty list.
            Self::Bytes(bytes) if bytes.is_empty() => Ok(&[]),
            other => Err(other.expected("an array")),
        }
    }

    /// A `PPtr<T>`: the file it lives in (0 = this one, else an external + 1) and its path ID.
    pub fn as_pptr(&self) -> Result<(i32, i64)> {
        Ok((self.field("m_FileID", Value::int)?, self.field("m_PathID", Value::as_i64)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors_name_the_field_and_refuse_wrong_kinds() {
        let value = Value::Object(vec![("id".into(), Value::UInt(300)), ("name".into(), Value::String("x".into()))]);
        assert_eq!(value.field("id", Value::int::<u16>).unwrap(), 300);
        let error = value.field("id", Value::int::<u8>).unwrap_err().to_string();
        assert!(error.contains("id") && error.contains("out of range"), "{error}");
        assert!(value.field("name", Value::as_f32).unwrap_err().to_string().contains("name"));
        assert!(value.get("missing").unwrap_err().to_string().contains("missing"));
        assert!(Value::Int(1).as_bool().unwrap());
        assert!(Value::Int(2).as_bool().is_err());
    }
}
