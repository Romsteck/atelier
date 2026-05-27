use crate::sqlx::{Pool, Postgres, query};
use crate::types::LogEntry;

pub async fn insert_batch(pool: &Pool<Postgres>, entries: &[LogEntry]) -> anyhow::Result<usize> {
    if entries.is_empty() {
        return Ok(0);
    }

    let mut ts: Vec<chrono::DateTime<chrono::Utc>> = Vec::with_capacity(entries.len());
    let mut service: Vec<String> = Vec::with_capacity(entries.len());
    let mut app_slug: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut level: Vec<String> = Vec::with_capacity(entries.len());
    let mut category: Vec<String> = Vec::with_capacity(entries.len());
    let mut message: Vec<String> = Vec::with_capacity(entries.len());
    let mut fields: Vec<Option<serde_json::Value>> = Vec::with_capacity(entries.len());
    let mut request_id: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut user_id: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut crate_name: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut module: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut function: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut file: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut line: Vec<Option<i32>> = Vec::with_capacity(entries.len());
    let mut app_version: Vec<Option<String>> = Vec::with_capacity(entries.len());
    let mut deploy_id: Vec<Option<String>> = Vec::with_capacity(entries.len());

    for e in entries {
        ts.push(e.timestamp);
        service.push(e.service.clone());
        app_slug.push(e.app_slug.clone());
        level.push(e.level.as_str().to_string());
        category.push(e.category.as_str().to_string());
        message.push(e.message.clone());
        fields.push(e.fields.clone());
        request_id.push(e.request_id.clone());
        user_id.push(e.user_id.clone());
        crate_name.push(e.source.crate_name.clone());
        module.push(e.source.module.clone());
        function.push(e.source.function.clone());
        file.push(e.source.file.clone());
        line.push(e.source.line.map(|n| n as i32));
        app_version.push(e.app_version.clone());
        deploy_id.push(e.deploy_id.clone());
    }

    let sql = r#"
        INSERT INTO events_log
            (ts, service, app_slug, level, category, message, fields,
             request_id, user_id, crate_name, module, function, file, line,
             app_version, deploy_id)
        SELECT *
        FROM UNNEST(
            $1::timestamptz[], $2::text[], $3::text[], $4::text[], $5::text[],
            $6::text[], $7::jsonb[],
            $8::text[], $9::text[], $10::text[], $11::text[], $12::text[],
            $13::text[], $14::int[],
            $15::text[], $16::text[]
        )
    "#;

    let n = query(sql)
        .bind(&ts[..])
        .bind(&service[..])
        .bind(&app_slug[..])
        .bind(&level[..])
        .bind(&category[..])
        .bind(&message[..])
        .bind(&fields[..])
        .bind(&request_id[..])
        .bind(&user_id[..])
        .bind(&crate_name[..])
        .bind(&module[..])
        .bind(&function[..])
        .bind(&file[..])
        .bind(&line[..])
        .bind(&app_version[..])
        .bind(&deploy_id[..])
        .execute::<&Pool<Postgres>>(pool)
        .await?
        .rows_affected();

    Ok(n as usize)
}
