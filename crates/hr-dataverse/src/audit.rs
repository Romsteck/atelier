//! Builder for `_dv_audit` insertions. Audit rows are written in the same
//! transaction as the data mutation that produced them, so the table is
//! the ground-truth history of every gateway-level operation.
//!
//! The schema is defined in [`crate::engine::INIT_METADATA_SQL`].

use hr_common::Identity;
use serde_json::Value;

use crate::query::QueryParam;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOp {
    Insert,
    Update,
    Delete,
    Restore,
}

impl AuditOp {
    pub fn as_sql(self) -> &'static str {
        match self {
            AuditOp::Insert => "INSERT",
            AuditOp::Update => "UPDATE",
            AuditOp::Delete => "DELETE",
            AuditOp::Restore => "RESTORE",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledAudit {
    pub sql: String,
    pub params: Vec<QueryParam>,
}

/// Build an `INSERT INTO _dv_audit …` for a single mutation. `before` and
/// `after` are full row snapshots (or `None` for INSERT-before / hard-DELETE-
/// after); `diff` is computed from those when both are provided.
pub fn build_audit_insert(
    table: &str,
    row_id: &Value,
    op: AuditOp,
    identity: &Identity,
    before: Option<&Value>,
    after: Option<&Value>,
) -> CompiledAudit {
    let actor_uuid = identity.actor_uuid();
    let actor_kind = identity.kind_str();
    let actor_label = identity.label();

    let diff = match (before, after) {
        (Some(b), Some(a)) => Some(json_diff(b, a)),
        _ => None,
    };

    // Two SQL shapes: the column count differs based on whether
    // `actor_uuid` is present. Embedding the column in the SQL only when
    // we have a value avoids binding a typed-int NULL that Postgres would
    // refuse to coerce into the UUID column type.
    let mut params: Vec<QueryParam> = Vec::new();
    params.push(QueryParam::Text(table.to_string()));
    params.push(QueryParam::Text(stringify_id(row_id)));
    params.push(QueryParam::Text(op.as_sql().into()));
    params.push(QueryParam::Text(actor_kind.into()));

    let sql = if let Some(u) = actor_uuid {
        params.push(QueryParam::Uuid(u.to_string()));
        params.push(QueryParam::Text(actor_label));
        params.push(json_param(before));
        params.push(json_param(after));
        params.push(json_param(diff.as_ref()));
        "INSERT INTO _dv_audit \
         (table_name, row_id, op, actor_kind, actor_uuid, actor_label, before, after, diff) \
         VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, $8::jsonb, $9::jsonb);"
            .to_string()
    } else {
        // System actor — actor_uuid stays NULL via column-default.
        params.push(QueryParam::Text(actor_label));
        params.push(json_param(before));
        params.push(json_param(after));
        params.push(json_param(diff.as_ref()));
        "INSERT INTO _dv_audit \
         (table_name, row_id, op, actor_kind, actor_label, before, after, diff) \
         VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7::jsonb, $8::jsonb);"
            .to_string()
    };
    CompiledAudit { sql, params }
}

fn stringify_id(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        _ => v.to_string(),
    }
}

fn json_param(v: Option<&Value>) -> QueryParam {
    // Bound as the textual JSON form so the SQL cast `::jsonb` succeeds.
    // A bare `QueryParam::Null` would bind as a typed-int NULL which
    // Postgres refuses to cast to jsonb (`ne peut pas convertir le type
    // bigint en jsonb`); the literal JSON value `null` (`Value::Null`)
    // round-trips cleanly through `::jsonb` and stays NULL-by-shape in
    // the column.
    match v {
        Some(j) => QueryParam::Text(j.to_string()),
        None => QueryParam::Text("null".into()),
    }
}

/// Compute a flat diff between two row JSON objects. Each top-level field
/// that differs is reported as `{ "field": { "before": …, "after": … } }`.
/// Fields present in only one side are still reported.
fn json_diff(before: &Value, after: &Value) -> Value {
    let mut out = serde_json::Map::new();
    let b = before.as_object();
    let a = after.as_object();
    if let (Some(bm), Some(am)) = (b, a) {
        let mut keys: std::collections::BTreeSet<&String> = bm.keys().collect();
        keys.extend(am.keys());
        for k in keys {
            let bv = bm.get(k).cloned().unwrap_or(Value::Null);
            let av = am.get(k).cloned().unwrap_or(Value::Null);
            if bv != av {
                out.insert(
                    k.clone(),
                    serde_json::json!({"before": bv, "after": av}),
                );
            }
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn id() -> Identity {
        Identity::user(Uuid::new_v4(), "alice")
    }

    #[test]
    fn audit_insert_has_nine_params() {
        let after = json!({"id": 1, "name": "x"});
        let c = build_audit_insert("orders", &json!(1), AuditOp::Insert, &id(), None, Some(&after));
        assert!(c.sql.contains("INSERT INTO _dv_audit"));
        assert!(c.sql.contains("actor_uuid"));
        assert_eq!(c.params.len(), 9);
    }

    #[test]
    fn audit_insert_system_omits_uuid_column() {
        // System identity has no uuid; the SQL must not list `actor_uuid`
        // (otherwise the bind would inject a typed-int NULL Postgres refuses
        // to coerce to UUID).
        let after = json!({"id": 1});
        let c = build_audit_insert("orders", &json!(1), AuditOp::Insert, &Identity::system(), None, Some(&after));
        assert!(!c.sql.contains("actor_uuid"));
        assert_eq!(c.params.len(), 8);
    }

    #[test]
    fn diff_only_for_update_with_both_snapshots() {
        let before = json!({"id": 1, "name": "old", "qty": 1});
        let after = json!({"id": 1, "name": "new", "qty": 1});
        let c = build_audit_insert(
            "orders",
            &json!(1),
            AuditOp::Update,
            &id(),
            Some(&before),
            Some(&after),
        );
        // The 9th param is the diff JSON.
        match &c.params[8] {
            QueryParam::Text(s) => {
                assert!(s.contains("\"name\""));
                assert!(s.contains("\"before\":\"old\""));
                assert!(s.contains("\"after\":\"new\""));
                assert!(!s.contains("\"qty\""), "diff must skip unchanged fields");
            }
            other => panic!("expected Text diff, got {:?}", other),
        }
    }

    #[test]
    fn delete_op_has_null_after() {
        let before = json!({"id": 1});
        let c = build_audit_insert(
            "orders",
            &json!(1),
            AuditOp::Delete,
            &id(),
            Some(&before),
            None,
        );
        // Bound as the literal JSON `null` (text) so the SQL cast
        // `::jsonb` succeeds — a bare `Null` bind would be typed as
        // bigint and rejected by Postgres.
        match &c.params[7] {
            QueryParam::Text(s) if s == "null" => {}
            other => panic!("after should be QueryParam::Text(\"null\"), got {:?}", other),
        }
    }

    #[test]
    fn system_identity_uses_short_form() {
        let c = build_audit_insert("orders", &json!(1), AuditOp::Insert, &Identity::system(), None, Some(&json!({})));
        // System form drops actor_uuid: param[3] is actor_kind, param[4] is
        // actor_label (the next column in the short SQL).
        assert!(matches!(&c.params[3], QueryParam::Text(s) if s == "system"));
        assert!(matches!(&c.params[4], QueryParam::Text(s) if s == "system"));
    }

    #[test]
    fn op_strings_match_check_constraint() {
        // The CHECK in INIT_METADATA_SQL is: op IN ('INSERT','UPDATE','DELETE','RESTORE').
        for op in [AuditOp::Insert, AuditOp::Update, AuditOp::Delete, AuditOp::Restore] {
            let s = op.as_sql();
            assert!(matches!(s, "INSERT" | "UPDATE" | "DELETE" | "RESTORE"));
        }
    }
}
