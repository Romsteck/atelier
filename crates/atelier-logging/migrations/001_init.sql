-- atelier-logging — bootstrap DDL (idempotent)
-- Run as `dataverse_admin` on the `atelier_logs` database.

-- Main partitioned table
CREATE TABLE IF NOT EXISTS events_log (
    id            BIGSERIAL                NOT NULL,
    ts            TIMESTAMPTZ              NOT NULL,
    service       TEXT                     NOT NULL,
    app_slug      TEXT,
    level         TEXT                     NOT NULL,
    category      TEXT                     NOT NULL,
    message       TEXT                     NOT NULL,
    fields        JSONB,
    request_id    TEXT,
    user_id       TEXT,
    crate_name    TEXT,
    module        TEXT,
    function      TEXT,
    file          TEXT,
    line          INTEGER,
    app_version   TEXT,
    deploy_id     TEXT,
    message_tsv   TSVECTOR                 GENERATED ALWAYS AS (to_tsvector('simple', message)) STORED,
    PRIMARY KEY (id, ts)
) PARTITION BY RANGE (ts);

-- Indices on parent (cascade to all partitions)
CREATE INDEX IF NOT EXISTS events_log_ts_idx          ON events_log (ts DESC);
CREATE INDEX IF NOT EXISTS events_log_level_idx       ON events_log (level) WHERE level IN ('warn','error');
CREATE INDEX IF NOT EXISTS events_log_app_idx         ON events_log (app_slug) WHERE app_slug IS NOT NULL;
CREATE INDEX IF NOT EXISTS events_log_service_idx     ON events_log (service);
CREATE INDEX IF NOT EXISTS events_log_request_id_idx  ON events_log (request_id) WHERE request_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS events_log_user_id_idx     ON events_log (user_id) WHERE user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS events_log_msg_tsv_idx     ON events_log USING GIN (message_tsv);
CREATE INDEX IF NOT EXISTS events_log_fields_gin      ON events_log USING GIN (fields jsonb_path_ops);

-- Per-day partition helper
CREATE OR REPLACE FUNCTION ensure_partition(target_date DATE)
RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE
    pname TEXT;
BEGIN
    pname := 'events_log_' || to_char(target_date, 'YYYY_MM_DD');
    EXECUTE format(
        'CREATE TABLE IF NOT EXISTS %I PARTITION OF events_log FOR VALUES FROM (%L) TO (%L);',
        pname, target_date::timestamptz, (target_date + 1)::timestamptz
    );
END
$$;

-- Retention helper (drop partitions strictly before cutoff)
CREATE OR REPLACE FUNCTION drop_partitions_before(cutoff DATE)
RETURNS INT LANGUAGE plpgsql AS $$
DECLARE
    r RECORD;
    cnt INT := 0;
BEGIN
    FOR r IN
        SELECT c.relname
        FROM pg_inherits i
        JOIN pg_class c ON c.oid = i.inhrelid
        JOIN pg_class p ON p.oid = i.inhparent
        WHERE p.relname = 'events_log'
          AND c.relname ~ '^events_log_\d{4}_\d{2}_\d{2}$'
          AND to_date(substring(c.relname FROM 'events_log_(\d{4}_\d{2}_\d{2})'), 'YYYY_MM_DD') < cutoff
    LOOP
        EXECUTE format('DROP TABLE IF EXISTS %I', r.relname);
        cnt := cnt + 1;
    END LOOP;
    RETURN cnt;
END
$$;

-- Bootstrap today, today+1, today+2 partitions
SELECT ensure_partition(CURRENT_DATE);
SELECT ensure_partition(CURRENT_DATE + 1);
SELECT ensure_partition(CURRENT_DATE + 2);

-- Permissions (writer role created separately by bootstrap code)
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'atelier_logs_writer') THEN
        EXECUTE 'GRANT CONNECT ON DATABASE ' || current_database() || ' TO atelier_logs_writer';
        EXECUTE 'GRANT USAGE ON SCHEMA public TO atelier_logs_writer';
        EXECUTE 'GRANT INSERT, SELECT ON events_log TO atelier_logs_writer';
        EXECUTE 'GRANT USAGE, SELECT ON SEQUENCE events_log_id_seq TO atelier_logs_writer';
    END IF;
END
$$;
