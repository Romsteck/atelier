use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Coalesce,
}

impl BinOp {
    pub fn precedence(self) -> u8 {
        // Lower = looser binding.
        match self {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 3,
            BinOp::Coalesce => 4,
            BinOp::Add | BinOp::Sub => 5,
            BinOp::Mul | BinOp::Div | BinOp::Mod => 6,
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }

    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::And | BinOp::Or)
    }

    pub fn is_arithmetic(self) -> bool {
        matches!(
            self,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
}

/// An untyped expression — output of the parser, input of the type checker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Literal(Literal),
    /// `this.col`, `customer.name`, plain `col` (sugar for `this.col`).
    Path(Vec<String>),
    /// `now()`, `today()`, `user()`, `app()`, `now`, `today`, `user`, `app` (no parens).
    Context(ContextRoot),
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnOp,
        rhs: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    },
    /// `if(cond, a, b)` is parsed as a Call but `IF` is preserved here when it appears as a
    /// reserved-form to give better type errors.
    If {
        cond: Box<Expr>,
        then_: Box<Expr>,
        else_: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextRoot {
    Now,
    Today,
    User,
    App,
}

impl ContextRoot {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "now" => Some(ContextRoot::Now),
            "today" => Some(ContextRoot::Today),
            "user" => Some(ContextRoot::User),
            "app" => Some(ContextRoot::App),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ContextRoot::Now => "now",
            ContextRoot::Today => "today",
            ContextRoot::User => "user",
            ContextRoot::App => "app",
        }
    }
}
