use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sqlx::{PgRow, Pool, Postgres, Row, query, query_as};
use crate::types::{LogCategory, LogEntry, LogLevel, LogSource};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub level: Option<String>, // comma-separated
    #[serde(default)]
    pub service: Option<String>, // comma-separated
    #[serde(default)]
    pub app_slug: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogStats {
    pub total: i64,
    pub by_level: Vec<LevelCount>,
    pub by_service: Vec<ServiceCount>,
    pub by_app: Vec<AppCount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LevelCount { pub level: String, pub count: i64 }

#[derive(Debug, Clone, Serialize)]
pub struct ServiceCount { pub service: String, pub count: i64 }

#[derive(Debug, Clone, Serialize)]
pub struct AppCount { pub app_slug: Option<String>, pub count: i64 }

fn csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect()
}

/// Holds filter values; SQL fragments are built in `apply_filters` and the
/// final query binds them in the same order.
#[derive(Default, Clone)]
struct Binds {
    values: Vec<BindValue>,
}

#[derive(Clone)]
enum BindValue {
    Str(String),
    StrVec(Vec<String>),
    Ts(DateTime<Utc>),
    I64(i64),
}

impl Binds {
    fn push_str(&mut self, s: String) -> usize {
        self.values.push(BindValue::Str(s));
        self.values.len()
    }
    fn push_strs(&mut self, v: Vec<String>) -> usize {
        self.values.push(BindValue::StrVec(v));
        self.values.len()
    }
    fn push_ts(&mut self, t: DateTime<Utc>) -> usize {
        self.values.push(BindValue::Ts(t));
        self.values.len()
    }
    fn push_i64(&mut self, n: i64) -> usize {
        self.values.push(BindValue::I64(n));
        self.values.len()
    }
}

fn apply_filters(q: &LogQuery, sql: &mut String, binds: &mut Binds) {
    if let Some(t) = q.since {
        let i = binds.push_ts(t);
        sql.push_str(&format!(" AND ts >= ${}", i));
    }
    if let Some(t) = q.until {
        let i = binds.push_ts(t);
        sql.push_str(&format!(" AND ts <= ${}", i));
    }
    if let Some(lvl) = &q.level {
        let v = csv(lvl);
        if !v.is_empty() {
            let i = binds.push_strs(v);
            sql.push_str(&format!(" AND level = ANY(${})", i));
        }
    }
    if let Some(svc) = &q.service {
        let v = csv(svc);
        if !v.is_empty() {
            let i = binds.push_strs(v);
            sql.push_str(&format!(" AND service = ANY(${})", i));
        }
    }
    if let Some(slug) = &q.app_slug {
        let i = binds.push_str(slug.clone());
        sql.push_str(&format!(" AND app_slug = ${}", i));
    }
    if let Some(rid) = &q.request_id {
        let i = binds.push_str(rid.clone());
        sql.push_str(&format!(" AND request_id = ${}", i));
    }
    if let Some(uid) = &q.user_id {
        let i = binds.push_str(uid.clone());
        sql.push_str(&format!(" AND user_id = ${}", i));
    }
    if let Some(cat) = &q.category {
        let i = binds.push_str(cat.clone());
        sql.push_str(&format!(" AND category = ${}", i));
    }
    if let Some(needle) = &q.q {
        if !needle.trim().is_empty() {
            let i = binds.push_str(format!("%{}%", needle));
            sql.push_str(&format!(" AND message ILIKE ${}", i));
        }
    }
}

pub async fn query_logs(pool: &Pool<Postgres>, q: &LogQuery) -> anyhow::Result<Vec<LogEntry>> {
    let mut sql = String::from(
        "SELECT id, ts, service, app_slug, level, category, message, fields, \
         request_id, user_id, crate_name, module, function, file, line, app_version, deploy_id \
         FROM events_log WHERE 1=1",
    );
    let mut binds = Binds::default();
    apply_filters(q, &mut sql, &mut binds);

    sql.push_str(" ORDER BY ts DESC, id DESC");
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let offset = q.offset.unwrap_or(0).max(0);
    let li = binds.push_i64(limit);
    sql.push_str(&format!(" LIMIT ${}", li));
    let oi = binds.push_i64(offset);
    sql.push_str(&format!(" OFFSET ${}", oi));

    let mut qb = query(&sql);
    for v in &binds.values {
        qb = match v.clone() {
            BindValue::Str(s) => qb.bind(s),
            BindValue::StrVec(v) => qb.bind(v),
            BindValue::Ts(t) => qb.bind(t),
            BindValue::I64(n) => qb.bind(n),
        };
    }
    let rows: Vec<PgRow> = qb.fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_entry(&row)?);
    }
    Ok(out)
}

pub async fn stats(pool: &Pool<Postgres>, q: &LogQuery) -> anyhow::Result<LogStats> {
    let total: i64 = {
        let mut sql = String::from("SELECT COUNT(*)::bigint FROM events_log WHERE 1=1");
        let mut binds = Binds::default();
        apply_filters(q, &mut sql, &mut binds);
        let mut qb = query_as::<_, (i64,)>(&sql);
        for v in &binds.values {
            qb = match v.clone() {
                BindValue::Str(s) => qb.bind(s),
                BindValue::StrVec(v) => qb.bind(v),
                BindValue::Ts(t) => qb.bind(t),
                BindValue::I64(n) => qb.bind(n),
            };
        }
        let (n,) = qb.fetch_one(pool).await?;
        n
    };

    let by_level: Vec<LevelCount> = {
        let mut sql = String::from("SELECT level, COUNT(*)::bigint FROM events_log WHERE 1=1");
        let mut binds = Binds::default();
        apply_filters(q, &mut sql, &mut binds);
        sql.push_str(" GROUP BY level ORDER BY 2 DESC");
        let mut qb = query_as::<_, (String, i64)>(&sql);
        for v in &binds.values {
            qb = match v.clone() {
                BindValue::Str(s) => qb.bind(s),
                BindValue::StrVec(v) => qb.bind(v),
                BindValue::Ts(t) => qb.bind(t),
                BindValue::I64(n) => qb.bind(n),
            };
        }
        qb.fetch_all(pool).await?.into_iter().map(|(l, c)| LevelCount { level: l, count: c }).collect()
    };

    let by_service: Vec<ServiceCount> = {
        let mut sql = String::from("SELECT service, COUNT(*)::bigint FROM events_log WHERE 1=1");
        let mut binds = Binds::default();
        apply_filters(q, &mut sql, &mut binds);
        sql.push_str(" GROUP BY service ORDER BY 2 DESC LIMIT 50");
        let mut qb = query_as::<_, (String, i64)>(&sql);
        for v in &binds.values {
            qb = match v.clone() {
                BindValue::Str(s) => qb.bind(s),
                BindValue::StrVec(v) => qb.bind(v),
                BindValue::Ts(t) => qb.bind(t),
                BindValue::I64(n) => qb.bind(n),
            };
        }
        qb.fetch_all(pool).await?.into_iter().map(|(s, c)| ServiceCount { service: s, count: c }).collect()
    };

    let by_app: Vec<AppCount> = {
        let mut sql = String::from("SELECT app_slug, COUNT(*)::bigint FROM events_log WHERE 1=1");
        let mut binds = Binds::default();
        apply_filters(q, &mut sql, &mut binds);
        sql.push_str(" GROUP BY app_slug ORDER BY 2 DESC LIMIT 50");
        let mut qb = query_as::<_, (Option<String>, i64)>(&sql);
        for v in &binds.values {
            qb = match v.clone() {
                BindValue::Str(s) => qb.bind(s),
                BindValue::StrVec(v) => qb.bind(v),
                BindValue::Ts(t) => qb.bind(t),
                BindValue::I64(n) => qb.bind(n),
            };
        }
        qb.fetch_all(pool).await?.into_iter().map(|(s, c)| AppCount { app_slug: s, count: c }).collect()
    };

    Ok(LogStats { total, by_level, by_service, by_app })
}

pub async fn by_request(pool: &Pool<Postgres>, request_id: &str) -> anyhow::Result<Vec<LogEntry>> {
    let sql = "SELECT id, ts, service, app_slug, level, category, message, fields, \
               request_id, user_id, crate_name, module, function, file, line, app_version, deploy_id \
               FROM events_log WHERE request_id = $1 ORDER BY ts ASC, id ASC LIMIT 5000";
    let rows: Vec<PgRow> = query(sql).bind(request_id).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_entry(&row)?);
    }
    Ok(out)
}

fn row_to_entry(row: &PgRow) -> anyhow::Result<LogEntry> {
    let id: i64 = row.try_get("id")?;
    let timestamp: DateTime<Utc> = row.try_get("ts")?;
    let service: String = row.try_get("service")?;
    let app_slug: Option<String> = row.try_get("app_slug")?;
    let level_s: String = row.try_get("level")?;
    let category_s: String = row.try_get("category")?;
    let message: String = row.try_get("message")?;
    let fields: Option<serde_json::Value> = row.try_get("fields").ok();
    let request_id: Option<String> = row.try_get("request_id")?;
    let user_id: Option<String> = row.try_get("user_id")?;
    let crate_name: Option<String> = row.try_get("crate_name")?;
    let module: Option<String> = row.try_get("module")?;
    let function: Option<String> = row.try_get("function")?;
    let file: Option<String> = row.try_get("file")?;
    let line: Option<i32> = row.try_get("line")?;
    let app_version: Option<String> = row.try_get("app_version")?;
    let deploy_id: Option<String> = row.try_get("deploy_id")?;

    Ok(LogEntry {
        id,
        timestamp,
        service,
        app_slug,
        level: LogLevel::from_str(&level_s).unwrap_or(LogLevel::Info),
        category: LogCategory::from_str(&category_s).unwrap_or(LogCategory::System),
        message,
        fields,
        request_id,
        user_id,
        source: LogSource {
            crate_name,
            module,
            function,
            file,
            line: line.and_then(|n| if n >= 0 { Some(n as u32) } else { None }),
        },
        app_version,
        deploy_id,
    })
}
