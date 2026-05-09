//! HomeRoute Dataverse expression language.
//!
//! Used by:
//! - Computed columns at schema definition (compiled to Postgres GENERATED when pure,
//!   evaluated at read-time otherwise).
//! - `$filter` query expressions on the REST gateway (compiled to parameterized SQL).
//! - Future row-level rights predicates.
//!
//! The library is pure no-IO — everything is deterministic given the AST + context.

pub mod ast;
pub mod checker;
pub mod error;
pub mod eval;
pub mod lexer;
pub mod parser;
pub mod sql;
pub mod types;

pub use ast::{BinOp, Expr, Literal, UnOp};
pub use checker::{Checker, Purity, TypedExpr};
pub use error::{Error, Result};
pub use eval::{Row, Value};
pub use sql::SqlEmitter;
pub use types::{NumKind, Type};

/// One-shot parse + type-check from source.
///
/// Returns the typed expression and its purity classification.
pub fn parse_and_check(
    source: &str,
    schema: &dyn checker::ColumnSchema,
) -> Result<(TypedExpr, Purity)> {
    let tokens = lexer::tokenize(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = Checker::new(schema);
    let typed = checker.check(&ast)?;
    let purity = checker.purity();
    Ok((typed, purity))
}
