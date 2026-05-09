use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NumKind {
    Int,
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Type {
    Number(NumKind),
    Text,
    Bool,
    Date,
    Timestamp,
    Uuid,
    Null,
}

impl Type {
    pub const INT: Type = Type::Number(NumKind::Int);
    pub const FLOAT: Type = Type::Number(NumKind::Float);

    pub fn is_number(self) -> bool {
        matches!(self, Type::Number(_))
    }

    /// Promote `a` and `b` to a common type for arithmetic / comparison. Returns
    /// `None` if no common type exists.
    pub fn unify(a: Type, b: Type) -> Option<Type> {
        use NumKind::*;
        use Type::*;
        match (a, b) {
            (x, y) if x == y => Some(x),
            (Null, x) | (x, Null) => Some(x),
            (Number(Int), Number(Float)) | (Number(Float), Number(Int)) => Some(Number(Float)),
            (Date, Timestamp) | (Timestamp, Date) => Some(Timestamp),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Type::Number(NumKind::Int) => "Int",
            Type::Number(NumKind::Float) => "Float",
            Type::Text => "Text",
            Type::Bool => "Bool",
            Type::Date => "Date",
            Type::Timestamp => "Timestamp",
            Type::Uuid => "Uuid",
            Type::Null => "Null",
        }
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
