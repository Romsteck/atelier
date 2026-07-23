//! Triage automatique des remontées plateforme.
//!
//! Une remontée d'agent (`issue_report` MCP / `POST /api/apps/{slug}/issues`)
//! est enfilée dans `pilot_triage` au lieu de l'ex-table `platform_issues`. Une
//! instance headless du chef de projet (run `scan.js` lecture seule + MCP scope
//! `pilot`, cf. [`crate::service::PilotService::run_triage_dispatcher`]) la lit,
//! investigue le code, et crée un item de backlog planifié (lane `ready`, ou
//! `attention` si doute). La **table est la file** : pas de VecDeque mémoire, le
//! dispatcher claim la plus ancienne ligne `pending` — restart-safe.
//!
//! Le prompt vit ICI (et non dans `atelier-api/pm_prompts.rs`) car le spawn est
//! dans `atelier-pilot`, qui ne dépend pas d'`atelier-api` (c'est l'inverse).

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::sqlx::{PgPool, PgRow, Row, query};

pub const TRIAGE_KINDS: &[&str] = &["error", "limitation", "suggestion"];
pub const TRIAGE_SEVERITIES: &[&str] = &["low", "medium", "high"];
pub const TRIAGE_AREAS: &[&str] = &[
    "mcp", "docs", "build", "deploy", "dataverse", "agent", "studio-ui", "platform", "other",
];

fn coerce<'a>(v: &'a str, allowed: &[&str], default: &'a str) -> &'a str {
    if allowed.contains(&v) { v } else { default }
}

/// Charge utile d'une remontée, stockée en JSONB dans `pilot_triage.payload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriagePayload {
    pub title: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub area: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub tried: String,
}

#[derive(Debug, Clone)]
pub struct TriageRow {
    pub id: i64,
    pub slug: String,
    pub payload: TriagePayload,
    pub attempts: i32,
}

/// Verdict structuré émis par le run de triage (fence JSON en fin de rapport).
#[derive(Debug, Clone)]
pub struct TriageOutcome {
    pub outcome: String,
    pub item_id: Option<i64>,
}

/// État agrégé du triage pour l'UI (bandeau « le chef de projet trie N
/// remontée(s) »). Diffusé en WS `pilot:triage` + `GET /api/pilot/triage`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TriageSummary {
    /// Remontées en file OU en cours (pending + running).
    pub active: i64,
    /// Remontée en cours de triage (au plus une, single-flight) : app + titre.
    pub running_slug: Option<String>,
    pub running_title: Option<String>,
}

#[derive(Clone)]
pub struct TriageStore {
    pool: PgPool,
}

impl TriageStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Enfile une remontée (status `pending`). `kind`/`area`/`severity` inconnus
    /// sont coercés vers leur défaut (mêmes enums que l'ex-store d'issues).
    pub async fn enqueue(
        &self,
        slug: &str,
        title: &str,
        kind: &str,
        area: &str,
        severity: &str,
        context: &str,
        tried: &str,
    ) -> anyhow::Result<i64> {
        let payload = json!({
            "title": title.trim(),
            "kind": coerce(kind, TRIAGE_KINDS, "error"),
            "area": coerce(area, TRIAGE_AREAS, "other"),
            "severity": coerce(severity, TRIAGE_SEVERITIES, "medium"),
            "context": context,
            "tried": tried,
        });
        let row = query(
            "INSERT INTO pilot_triage (slug, payload) VALUES ($1, $2) RETURNING id",
        )
        .bind(slug)
        .bind(payload)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id")?)
    }

    /// Claim la plus ancienne remontée `pending` : passe `running` et incrémente
    /// `attempts` (au CLAIM, pas au finish — crash-safe : un triage qui tue le
    /// service en boucle atteint le fallback en 2 boots). `SKIP LOCKED` par
    /// prudence même si le dispatcher est single-flight.
    pub async fn claim_oldest(&self) -> anyhow::Result<Option<TriageRow>> {
        let row = query(
            "UPDATE pilot_triage SET status='running', attempts=attempts+1, updated_at=now() \
             WHERE id = (SELECT id FROM pilot_triage WHERE status='pending' ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED) \
             RETURNING id, slug, payload, attempts",
        )
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(row_to_triage).transpose()
    }

    pub async fn settle_done(
        &self,
        id: i64,
        item_id: Option<i64>,
        outcome: &str,
    ) -> anyhow::Result<()> {
        query(
            "UPDATE pilot_triage SET status='done', outcome=$2, backlog_item_id=$3, error=NULL, updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(outcome)
        .bind(item_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn settle_failed(&self, id: i64, error: &str) -> anyhow::Result<()> {
        query("UPDATE pilot_triage SET status='failed', error=$2, updated_at=now() WHERE id=$1")
            .bind(id)
            .bind(error)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remet une tentative échouée en file (`running`→`pending`) pour un retry
    /// immédiat par le dispatcher — `attempts` a déjà été incrémenté au claim.
    pub async fn requeue(&self, id: i64) -> anyhow::Result<()> {
        query("UPDATE pilot_triage SET status='pending', updated_at=now() WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Y a-t-il du travail en attente/en cours ? (kick conditionnel du dispatcher.)
    pub async fn has_pending(&self) -> anyhow::Result<bool> {
        let row = query("SELECT 1 FROM pilot_triage WHERE status='pending' LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    /// Snapshot pour le bandeau UI : nombre de remontées actives (pending +
    /// running) et l'app/titre de celle en cours de triage.
    pub async fn summary(&self) -> anyhow::Result<TriageSummary> {
        let active: i64 =
            query("SELECT count(*) AS n FROM pilot_triage WHERE status IN ('pending','running')")
                .fetch_one(&self.pool)
                .await?
                .try_get("n")?;
        let running = query(
            "SELECT slug, payload->>'title' AS title FROM pilot_triage WHERE status='running' ORDER BY id LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let (running_slug, running_title) = match running {
            Some(r) => (r.try_get("slug").ok(), r.try_get("title").ok().flatten()),
            None => (None, None),
        };
        Ok(TriageSummary {
            active,
            running_slug,
            running_title,
        })
    }

    /// Au boot : les triages `running` (run tué par un restart) repassent
    /// `pending` pour être rejoués. `attempts` conservé.
    pub async fn requeue_interrupted(&self) -> anyhow::Result<u64> {
        Ok(
            query("UPDATE pilot_triage SET status='pending', updated_at=now() WHERE status='running'")
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }

    /// Migration one-shot des remontées `open` de l'ex-table `platform_issues`
    /// vers la file de triage, puis marque-les `resolved`. Idempotent (ne repique
    /// que les `open`) et gardé par `to_regclass` (no-op si la table n'existe pas,
    /// ex. install neuve). Retourne le nombre migré.
    pub async fn migrate_platform_issues(&self) -> anyhow::Result<u64> {
        // `to_regclass` renvoie le type `regclass` — caster en text pour le
        // décoder en Option<String> (NULL si la table n'existe pas).
        let exists = query("SELECT to_regclass('public.platform_issues')::text AS t")
            .fetch_one(&self.pool)
            .await?
            .try_get::<Option<String>, _>("t")?
            .is_some();
        if !exists {
            return Ok(0);
        }
        let rows = query(
            "SELECT slug, kind, area, severity, title, \
             COALESCE(context,'') AS context, COALESCE(tried,'') AS tried \
             FROM platform_issues WHERE status='open' ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut migrated = 0u64;
        for r in &rows {
            let slug: String = r.try_get("slug")?;
            self.enqueue(
                &slug,
                &r.try_get::<String, _>("title")?,
                &r.try_get::<String, _>("kind")?,
                &r.try_get::<String, _>("area")?,
                &r.try_get::<String, _>("severity")?,
                &r.try_get::<String, _>("context")?,
                &r.try_get::<String, _>("tried")?,
            )
            .await?;
            migrated += 1;
        }
        if migrated > 0 {
            query(
                "UPDATE platform_issues SET status='resolved', note='migrée backlog Pilote', updated_at=now() WHERE status='open'",
            )
            .execute(&self.pool)
            .await?;
        }
        Ok(migrated)
    }
}

fn row_to_triage(row: &PgRow) -> anyhow::Result<TriageRow> {
    let payload: serde_json::Value = row.try_get("payload")?;
    Ok(TriageRow {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        payload: serde_json::from_value(payload).unwrap_or(TriagePayload {
            title: "Remontée sans titre".into(),
            kind: "error".into(),
            area: "other".into(),
            severity: "medium".into(),
            context: String::new(),
            tried: String::new(),
        }),
        attempts: row.try_get("attempts")?,
    })
}

/// Priorité dérivée de la sévérité de la remontée (miroir de la doctrine du
/// prompt — sert de repli si l'agent ne score pas lui-même).
pub fn severity_to_priority(sev: &str) -> &'static str {
    match sev {
        "high" => "high",
        "low" => "low",
        _ => "medium",
    }
}

/// kind remontée → kind backlog.
pub fn kind_to_backlog_kind(kind: &str) -> &'static str {
    match kind {
        "error" => "bug",
        _ => "improvement", // limitation | suggestion
    }
}

/// Prompt du run de triage headless (chef de projet, lecture seule). Patron
/// [`crate::service::build_item_prompt`] : un `format!` unique, verdict en fence
/// JSON parsé par [`triage_outcome`].
pub fn build_triage_prompt(row: &TriageRow) -> String {
    let p = &row.payload;
    let ctx = if p.context.trim().is_empty() { "(aucun)" } else { p.context.trim() };
    let tried = if p.tried.trim().is_empty() { "(aucun)" } else { p.tried.trim() };
    format!(
        r#"[TRIAGE AUTONOME — CHEF DE PROJET ATELIER]
Tu es le chef de projet de la plateforme Atelier. Un agent (interactif ou autonome) a envoyé une REMONTÉE de friction PLATEFORME. Ton rôle : décider quoi en faire et, si c'est actionnable, créer un item de backlog PLANIFIÉ. Tu es en LECTURE SEULE sur le code (Read/Glob/Grep) et tu écris UNIQUEMENT via les tools MCP backlog_*.

REMONTÉE #{id}
- App source : {slug}
- Type : {kind}   (error = cassé · limitation = bride · suggestion = idée)
- Domaine : {area}
- Sévérité : {severity}
- Titre : {title}
- Contexte : {context}
- Déjà tenté : {tried}

Procédure :
1. DÉDUP — appelle backlog_list (scope pertinent) et cherche un item OUVERT (lane ready/in_progress/attention) qui couvre déjà cette friction. Si oui : enrichis-le via backlog_update (précise la reproduction, ajoute l'app source au contexte) et TERMINE en outcome="duplicate" (item_id = l'id existant). NE crée PAS de doublon.
2. INVESTIGUE — localise et valide la friction dans le code (lecture seule). Une remontée plateforme concerne Atelier lui-même (tool MCP, doc, build/deploy, dataverse, agent) — le plus souvent scope 'atelier'. Si la cause est en réalité côté app, scope = le slug de l'app.
3. ACTIONNABLE → backlog_add : scope ('atelier' ou slug app), kind ({kind}→ bug si error, sinon improvement), priority dérivée de la sévérité ({severity} → high/medium/low), effort estimé (xs..xl), title clair, request (reformulation nette du besoin), description CITANT la remontée d'origine (#{id}, app {slug}, contexte, ce qui a été tenté), et surtout un PLAN d'implémentation complet et concret (fichiers, approche, étapes de validation). lane par défaut = ready. Termine en outcome="planned" (item_id = l'id créé).
4. DOUTE — si une décision produit, un risque, une ambiguïté ou un arbitrage te bloque : crée quand même l'item via backlog_add avec needs_user=true, une reason courte, et des `questions` en QCM. Chaque question = une phrase courte + une liste `options` de 2 à 4 choix mutuellement exclusifs, la RECOMMANDÉE en premier (suffixée « (recommandé) ») : Romain répond en un clic, sans deviner les possibilités — c'est TOI qui proposes les options. UNE décision par question, 2 questions max. Termine en outcome="needs_user" (item_id = l'id créé).
5. NON PERTINENT — si la remontée est un faux positif, hors périmètre plateforme, ou déjà résolu dans le code : n'écris rien et termine en outcome="rejected".

Interdits : issue_report (tu ES le triage), notify_user, toute modification de code, toute création hors de cette remontée.

Ta réponse finale doit se terminer par un bloc JSON valide de cette forme (sans texte après) :
```json
{{"triage":{{"outcome":"planned|needs_user|duplicate|rejected","item_id":123}}}}
```
item_id = l'id de l'item créé ou enrichi (omis/null pour rejected)."#,
        id = row.id,
        slug = row.slug,
        kind = p.kind,
        area = p.area,
        severity = p.severity,
        title = p.title,
        context = ctx,
        tried = tried,
    )
}

/// Parse le verdict en fin de rapport (dernier fence ```json{{"triage":…}}```).
/// Patron [`crate::service::worker_needs_user`]. Fence absent/illisible → None
/// (traité comme tentative échouée → retry sur la branche dédup, pas de doublon).
pub fn triage_outcome(text: &str) -> Option<TriageOutcome> {
    let json_text = text
        .rsplit_once("```json")
        .and_then(|(_, tail)| tail.split_once("```").map(|(body, _)| body.trim()))?;
    let value: serde_json::Value = serde_json::from_str(json_text).ok()?;
    let triage = value.get("triage")?;
    let outcome = triage.get("outcome").and_then(serde_json::Value::as_str)?;
    if !["planned", "needs_user", "duplicate", "rejected"].contains(&outcome) {
        return None;
    }
    Some(TriageOutcome {
        outcome: outcome.to_string(),
        item_id: triage.get("item_id").and_then(serde_json::Value::as_i64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_planned_outcome() {
        let text = "bla bla\n```json\n{\"triage\":{\"outcome\":\"planned\",\"item_id\":42}}\n```";
        let o = triage_outcome(text).expect("outcome");
        assert_eq!(o.outcome, "planned");
        assert_eq!(o.item_id, Some(42));
    }

    #[test]
    fn parses_rejected_without_item() {
        let text = "```json\n{\"triage\":{\"outcome\":\"rejected\"}}\n```";
        let o = triage_outcome(text).expect("outcome");
        assert_eq!(o.outcome, "rejected");
        assert_eq!(o.item_id, None);
    }

    #[test]
    fn missing_fence_is_none() {
        assert!(triage_outcome("no json here").is_none());
        assert!(triage_outcome("```json\n{\"triage\":{\"outcome\":\"bogus\"}}\n```").is_none());
    }

    #[test]
    fn mappings() {
        assert_eq!(kind_to_backlog_kind("error"), "bug");
        assert_eq!(kind_to_backlog_kind("suggestion"), "improvement");
        assert_eq!(severity_to_priority("high"), "high");
        assert_eq!(severity_to_priority("medium"), "medium");
    }
}
