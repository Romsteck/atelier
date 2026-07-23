#!/usr/bin/env bash
# Detached Atelier autonomous worker. The model only edits the source tree;
# checkpoint/deploy/health/commit/rollback/report remain deterministic here.
set -uo pipefail

# PATH (WHY) : l'unité `systemd-run --uid=romain` démarre sur le PATH systemd nu —
# `~/.cargo/bin` (cargo) et `~/.local/bin` en sont absents. Sans ce prepend,
# `make deploy-local` meurt en « cargo: command not found »… et le rollback
# re-déploie avec le même PATH et re-meurt pareil → faux `rollback_failed`.
export PATH="${HOME:-/home/romain}/.cargo/bin:${HOME:-/home/romain}/.local/bin:$PATH"

payload=${1:-}
if [[ -z "$payload" || ! -f "$payload" ]]; then
  exit 64
fi

runtime_dir=$(dirname "$payload")
run_id=$(/usr/bin/jq -r '.run_id' "$payload")
item_id=$(/usr/bin/jq -r '.item_id' "$payload")
title=$(/usr/bin/jq -r '.title' "$payload")
root=$(/usr/bin/jq -r '.root' "$payload")
api=$(/usr/bin/jq -r '.api' "$payload")
node=$(/usr/bin/jq -r '.node' "$payload")
worker=$(/usr/bin/jq -r '.worker' "$payload")
config_dir=$(/usr/bin/jq -r '.config_dir' "$payload")
# Secret de report en variable shell NON exportée : jamais dans l'env des enfants,
# jamais en argv (visible /proc/*/cmdline pour tout process romain — y compris un
# reliquat lancé par l'agent via Bash).
secret=$(/usr/bin/jq -r '.secret // ""' "$payload")
progress_api=$(/usr/bin/jq -r '.progress_api // ""' "$payload")
[[ -z "$progress_api" ]] && progress_api="${api%atelier-report}atelier-progress"
phase_file="$runtime_dir/$run_id.phase"
marker="$runtime_dir/$run_id.report.json"
transcript="$runtime_dir/$run_id.ndjson"
stderr_log="$runtime_dir/$run_id.stderr.log"
run_log="$runtime_dir/$run_id.log"
checkpoint_sha=""
sha_before=""
reported=0
phase=bootstrap

write_report() {
  local status=$1 reason=$2 error_text=$3 commit_sha=$4 report_text=$5
  local tmp="$marker.tmp"
  # Marker durable SANS secret : la réconciliation Rust (reconcile_atelier_worker) le lit
  # directement sur disque — le secret n'authentifie que le POST loopback. Le champ doit
  # exister (serde String non optionnelle côté AtelierWorkerReport) mais reste vide.
  /usr/bin/python3 - "$run_id" "$item_id" "$status" "$reason" "$error_text" "$commit_sha" "$report_text" "$checkpoint_sha" "$sha_before" "$tmp" <<'PY'
import json, os, sys
run_id, item_id, status, reason, error, commit, report, checkpoint, before, target = sys.argv[1:]
try:
    item = int(item_id)
except ValueError:
    item = 0
out = {
    "run_id": run_id, "secret": "",
    "item_id": item, "status": status,
    "commit_sha": commit or None, "report": report or None,
    "error": error or None, "failure_reason": reason or None,
    "checkpoint_sha": checkpoint or None,
    "git_sha_before": before or None,
}
with open(target, "w", encoding="utf-8") as fh:
    json.dump(out, fh, ensure_ascii=False)
    fh.flush(); os.fsync(fh.fileno())
os.chmod(target, 0o600)
PY
  mv -f "$tmp" "$marker"
  reported=1
  # Corps du POST = marker + secret réinjecté. Le secret ne transite NI par argv NI par
  # un fichier persistant : printf est un builtin (pas de /proc/cmdline), python le lit
  # sur stdin, et curl reçoit le JSON par heredoc/stdin (`--data-binary @-`).
  local body
  body=$(printf '%s' "$secret" | /usr/bin/python3 -c '
import json, sys
secret = sys.stdin.read()
with open(sys.argv[1], encoding="utf-8") as fh:
    d = json.load(fh)
d["secret"] = secret
print(json.dumps(d, ensure_ascii=False))
' "$marker")
  for _ in $(seq 1 20); do
    if /usr/bin/curl -fsS --connect-timeout 3 --max-time 10 \
      -H 'Content-Type: application/json' --data-binary @- "$api" >/dev/null <<CURL_BODY
$body
CURL_BODY
    then
      rm -f "$marker" "$transcript" "$stderr_log" "$run_log" "$payload" "$phase_file"
      return 0
    fi
    sleep 3
  done
  return 1
}

on_exit() {
  local code=$?
  if [[ $reported -eq 0 ]]; then
    write_report failed agent_error "Worker Atelier interrompu pendant: $phase (exit $code)" "" "" || true
  fi
}
trap on_exit EXIT

# Jalon de phase : fichier durable (relu par la réconciliation post-restart pour
# ré-afficher l'overlay de maintenance au bon endroit) + POST fire-and-forget
# (relayé en WS `platform:maintenance` aux UIs ouvertes). Jamais bloquant :
# pendant le restart d'Atelier le POST échoue, c'est attendu. Le secret passe
# par stdin (printf builtin), jamais en argv.
post_progress() {
  local ph=$1
  printf '%s' "$ph" > "$phase_file" 2>/dev/null || true
  printf '%s' "$secret" | /usr/bin/python3 -c '
import json, sys
print(json.dumps({"run_id": sys.argv[1], "phase": sys.argv[2], "secret": sys.stdin.read()}))
' "$run_id" "$ph" 2>/dev/null | /usr/bin/curl -fsS --connect-timeout 2 --max-time 5 \
    -H 'Content-Type: application/json' --data-binary @- "$progress_api" >/dev/null 2>&1 || true
}

phase=preflight
for bin in /usr/bin/git /usr/bin/jq /usr/bin/python3 /usr/bin/curl /usr/bin/timeout "$node" "$worker"; do
  if [[ ! -x "$bin" && ! -f "$bin" ]]; then
    write_report failed spawn_error "Prérequis absent: $bin" "" "" || true
    exit 1
  fi
done
mkdir -p "$config_dir"
chmod 700 "$config_dir"
cd "$root" || { write_report failed spawn_error "Dépôt Atelier absent: $root" "" "" || true; exit 1; }

# Init du worker construit AVANT la purge du payload, livré plus bas par stdin (aucun
# fichier init sur disque : il porte l'oauth_token). Il ne contient PAS le secret de report.
init_json=$(/usr/bin/python3 - "$payload" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as fh:
    p = json.load(fh)
init = {
    "prompt": p["prompt"], "cwd": p["root"], "writeRoot": p["root"],
    "model": p["model"], "effort": p["effort"],
    "oauthToken": p.get("oauth_token"), "mcpEndpoint": None, "mcpToken": None,
}
json.dump(init, sys.stdout, ensure_ascii=False)
PY
)
if [[ -z "$init_json" ]]; then
  write_report failed spawn_error "Init worker inconstructible depuis le payload" "" "" || true
  exit 1
fi

phase=checkpoint
post_progress checkpoint
dirty=$(/usr/bin/git status --porcelain=v1 --untracked-files=all)
if [[ -n "$dirty" ]]; then
  /usr/bin/git add -A || { write_report failed commit_failed "git add checkpoint échoué" "" "" || true; exit 1; }
  body=$(printf '%s\n' "$dirty" | head -n 200)
  # hooksPath neutralisé (WHY) : un hook du dépôt (pré-existant ou posé par un run
  # antérieur) s'exécuterait EN ROMAIN pendant cette phase déterministe.
  /usr/bin/git -c core.hooksPath=/dev/null -c user.name='Romain (checkpoint)' -c user.email='pilot-checkpoint@atelier.local' \
    commit -m "chore(atelier): snapshot pré-autonome" -m "Fichiers avant run Pilote:
$body" >/dev/null || { write_report failed commit_failed "commit checkpoint échoué" "" "" || true; exit 1; }
  checkpoint_sha=$(/usr/bin/git rev-parse HEAD)
  # Push immédiat du snapshot (best-effort, jamais bloquant) : le travail humain
  # capturé ne doit pas exister en un seul exemplaire si la suite du run tourne mal.
  /usr/bin/timeout 60 /usr/bin/git push >>"$run_log" 2>&1 || true
fi
sha_before=$(/usr/bin/git rev-parse HEAD) || { write_report failed commit_failed "HEAD Atelier illisible" "" "" || true; exit 1; }

# Purge des secrets AVANT l'agent (WHY) : le tool Read du worker n'est pas confiné au
# workspace — rien de secret (secret de report, oauth_token, prompt) ne doit rester
# lisible sur disque pendant le run. On ne supprime PAS le fichier : la réconciliation
# Rust y relit checkpoint_sha/git_sha_before pour restaurer la source si l'unité meurt
# sans rapport. Il ne reste donc QUE l'état git, en remplacement atomique 0600.
/usr/bin/python3 - "$payload" "$checkpoint_sha" "$sha_before" <<'PY'
import json, os, sys
path, checkpoint, before = sys.argv[1:]
state = {"checkpoint_sha": checkpoint or None, "git_sha_before": before}
tmp = path + ".tmp"
with open(tmp, "w", encoding="utf-8") as fh:
    json.dump(state, fh, ensure_ascii=False)
    fh.flush(); os.fsync(fh.fileno())
os.chmod(tmp, 0o600)
os.replace(tmp, path)
PY
if [[ $? -ne 0 ]]; then
  write_report failed spawn_error "Purge des secrets du payload échouée" "" "" || true
  exit 1
fi

phase=agent
post_progress agent
# stderr SÉPARÉ du transcript (JAMAIS 2>&1 : une seule ligne non-JSON dans le NDJSON
# rendait l'ancien parsing en bloc `jq -rs` aveugle). Init par stdin via printf builtin.
# Timeout dur : TERM à 5400 s (le worker émet alors un verdict `cancelled` typé et sort
# proprement), KILL 30 s plus tard si besoin.
printf '%s\n' "$init_json" | /usr/bin/timeout -k 30 5400 "$node" "$worker" > "$transcript" 2> "$stderr_log"
agent_status=$?
if [[ $agent_status -eq 124 || $agent_status -eq 137 ]]; then
  /usr/bin/git reset --hard "$sha_before" >/dev/null 2>&1 || true
  /usr/bin/git clean -fd >/dev/null 2>&1 || true
  write_report failed timeout "Agent Atelier hors délai (5400 s); source restaurée. stderr: $(tail -c 1000 "$stderr_log" 2>/dev/null || true)" "" "" || true
  exit 1
fi

# Extraction UNIQUE et tolérante ligne-à-ligne (rapport final + dernier code d'erreur +
# outcome structuré) : une ligne parasite est ignorée, jamais bloquante pour le reste.
extract=$(/usr/bin/python3 - "$transcript" <<'PY'
import json, re, sys
report, reason = "", ""
try:
    fh = open(sys.argv[1], encoding="utf-8", errors="replace")
except OSError:
    fh = None
if fh is not None:
    with fh:
        for line in fh:
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(event, dict):
                continue
            if event.get("t") == "final_report":
                report = event.get("text") or ""
            elif event.get("t") == "error" and event.get("code"):
                reason = event["code"]
outcome = ""
blocks = re.findall(r"```json\s*(\{.*?\})\s*```", report, flags=re.DOTALL)
if blocks:
    try:
        outcome = json.loads(blocks[-1]).get("pilot", {}).get("outcome", "") or ""
    except (json.JSONDecodeError, AttributeError):
        outcome = ""
json.dump({"report": report[:8000], "reason": reason or "agent_error", "outcome": outcome},
          sys.stdout, ensure_ascii=False)
PY
)
report_text=$(printf '%s' "$extract" | /usr/bin/jq -r '.report // ""' 2>/dev/null || true)
reason=$(printf '%s' "$extract" | /usr/bin/jq -r '.reason // "agent_error"' 2>/dev/null || true)
outcome=$(printf '%s' "$extract" | /usr/bin/jq -r '.outcome // ""' 2>/dev/null || true)
reason=${reason:-agent_error}

if [[ $agent_status -ne 0 || -z "$report_text" ]]; then
  /usr/bin/git reset --hard "$sha_before" >/dev/null 2>&1 || true
  /usr/bin/git clean -fd >/dev/null 2>&1 || true
  write_report failed "$reason" "Agent Atelier en échec (exit $agent_status). stderr: $(tail -c 1500 "$stderr_log" 2>/dev/null || true)" "" "$report_text" || true
  exit 1
fi

# The detached worker has no MCP access. Honour the same structured
# needs-user contract as app workers before any build, deploy or commit.
if [[ "$outcome" == "needs_user" ]]; then
  /usr/bin/git reset --hard "$sha_before" >/dev/null 2>&1 || true
  /usr/bin/git clean -fd >/dev/null 2>&1 || true
  write_report needs_user needs_user "Décision utilisateur requise" "" "$report_text" || true
  exit 0
fi

phase=head_check
if [[ $(/usr/bin/git rev-parse HEAD) != "$sha_before" ]]; then
  /usr/bin/git reset --hard "$sha_before" >/dev/null 2>&1 || true
  /usr/bin/git clean -fd >/dev/null 2>&1 || true
  write_report failed head_moved "L’agent a créé un commit clandestin; HEAD restauré" "" "$report_text" || true
  exit 1
fi
if [[ -z $(/usr/bin/git status --porcelain=v1 --untracked-files=all) ]]; then
  write_report success "" "" "" "$report_text" || true
  exit 0
fi

# Diagnostic rollback (WHY) : en double-échec (deploy KO puis rollback KO) le run devient
# `rollback_failed` critique — sans capture, impossible de savoir POURQUOI. Tout le output
# deploy/rollback va dans $run_log, dont un tail est embarqué dans les reports d'échec.
rollback_and_redeploy() {
  post_progress rollback
  {
    echo "--- rollback $(date -Is) ---"
    /usr/bin/git reset --hard "$sha_before" && /usr/bin/git clean -fd
  } >>"$run_log" 2>&1 || return 1
  make deploy-local >>"$run_log" 2>&1
}
deploy_tail() { tail -c 1500 "$run_log" 2>/dev/null || true; }

phase=deploy
post_progress deploy
if ! make deploy-local >>"$run_log" 2>&1; then
  if rollback_and_redeploy; then
    write_report failed deploy_failed "make deploy-local a échoué; source restaurée et plateforme redéployée. Log: $(deploy_tail)" "" "$report_text" || true
  else
    write_report failed rollback_failed "deploy et rollback/redéploiement ont échoué. Log: $(deploy_tail)" "" "$report_text" || true
  fi
  exit 1
fi

phase=healthcheck
# Pas de jalon POSTé ici : `make deploy-local` vient de redémarrer Atelier, le
# premier POST joignable est celui de la phase commit (le fichier suffit).
printf '%s' healthcheck > "$phase_file" 2>/dev/null || true
healthy=0
for _ in $(seq 1 60); do
  if /usr/bin/curl -fsS --max-time 3 http://127.0.0.1:4100/api/health >/dev/null \
    && /usr/bin/curl -fsS --max-time 3 http://127.0.0.1:4100/ >/dev/null; then
    healthy=1; break
  fi
  sleep 1
done
if [[ $healthy -ne 1 ]]; then
  if rollback_and_redeploy; then
    write_report failed healthcheck_failed "Atelier ne répond pas après deploy; source restaurée" "" "$report_text" || true
  else
    write_report failed rollback_failed "healthcheck et rollback/redéploiement ont échoué. Log: $(deploy_tail)" "" "$report_text" || true
  fi
  exit 1
fi

phase=commit
post_progress commit
/usr/bin/git add -A
# hooksPath neutralisé (WHY) : un hook posé par l'agent via Bash pendant le run
# s'exécuterait sinon EN ROMAIN ici, à la phase déterministe.
if ! /usr/bin/git -c core.hooksPath=/dev/null -c user.name='Atelier Pilote' -c user.email='pilot@atelier.local' \
  commit -m "auto(atelier): $title (backlog:$item_id)" >/dev/null; then
  if rollback_and_redeploy; then
    write_report failed commit_failed "commit final Atelier échoué; source restaurée" "" "$report_text" || true
  else
    write_report failed rollback_failed "commit final et rollback/redéploiement ont échoué. Log: $(deploy_tail)" "" "$report_text" || true
  fi
  exit 1
fi
commit_sha=$(/usr/bin/git rev-parse HEAD)
# Push best-effort (origin GitHub, en romain) : jamais bloquant — un échec réseau
# laisse le commit local, visible « en attente de push » dans la bande des dépôts.
/usr/bin/timeout 60 /usr/bin/git push >>"$run_log" 2>&1 || true
write_report success "" "" "$commit_sha" "$report_text" || true
exit 0
