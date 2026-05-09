use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::port_registry::PortRegistry;
use crate::registry::AppRegistry;
use crate::types::{AppState, Application};

const HEALTH_INTERVAL: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
const WATCH_INTERVAL: Duration = Duration::from_secs(2);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
// Délai d'attente que l'unité atteigne ActiveState=inactive après SIGTERM gracieux.
// Au-delà, on envoie SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(30);
// Délai d'attente après SIGKILL avant d'abandonner.
const STOP_KILL_GRACE: Duration = Duration::from_secs(5);
// Intervalle de polling de ActiveState pendant un stop.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(200);
const RESTART_RESET_AFTER: Duration = Duration::from_secs(300);
const MAX_RESTARTS_PER_MIN: u32 = 10;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const SLICE: &str = "hr-apps.slice";

fn unit_name(slug: &str) -> String {
    format!("hr-app-{slug}.service")
}

/// Status snapshot of a supervised app process.
#[derive(Debug, Clone)]
pub struct ProcessStatus {
    pub pid: Option<u32>,
    pub state: AppState,
    pub port: u16,
    pub uptime_secs: u64,
    pub restart_count: u32,
}

/// Internal record for one supervised process.
struct SupervisedProcess {
    port: u16,
    pid: Option<u32>,
    state: AppState,
    started_at: Option<Instant>,
    restart_count: u32,
    restart_history: Vec<Instant>,
    next_backoff: Duration,
    last_start: Option<Instant>,
    /// Background task watching the systemd unit + driving respawn.
    watcher: Option<JoinHandle<()>>,
    /// Background task running the health-check loop.
    health: Option<JoinHandle<()>>,
    /// Whether stop() was requested (suppresses respawn).
    stop_requested: bool,
}

impl SupervisedProcess {
    fn new(port: u16) -> Self {
        Self {
            port,
            pid: None,
            state: AppState::Stopped,
            started_at: None,
            restart_count: 0,
            restart_history: Vec::new(),
            next_backoff: Duration::from_secs(1),
            last_start: None,
            watcher: None,
            health: None,
            stop_requested: false,
        }
    }

    fn status(&self) -> ProcessStatus {
        let uptime_secs = self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        ProcessStatus {
            pid: self.pid,
            state: self.state,
            port: self.port,
            uptime_secs,
            restart_count: self.restart_count,
        }
    }
}

type ProcessMap = Arc<RwLock<HashMap<String, Arc<Mutex<SupervisedProcess>>>>>;

/// Supervises HomeRoute application processes via systemd transient units.
#[derive(Clone)]
pub struct AppSupervisor {
    pub registry: AppRegistry,
    pub port_registry: PortRegistry,
    processes: ProcessMap,
    app_state_tx: tokio::sync::broadcast::Sender<hr_common::events::AppStateEvent>,
}

impl AppSupervisor {
    pub fn new(
        registry: AppRegistry,
        port_registry: PortRegistry,
        app_state_tx: tokio::sync::broadcast::Sender<hr_common::events::AppStateEvent>,
    ) -> Self {
        Self {
            registry,
            port_registry,
            processes: Arc::new(RwLock::new(HashMap::new())),
            app_state_tx,
        }
    }

    /// Start an app: assign port, launch systemd transient unit, attach watcher + health loop.
    pub async fn start(&self, slug: &str) -> Result<()> {
        let mut app = self
            .registry
            .get(slug)
            .await
            .ok_or_else(|| anyhow!("app not found: {slug}"))?;

        if app.port == 0 {
            let port = self
                .port_registry
                .assign(slug)
                .await
                .with_context(|| format!("assigning port for {slug}"))?;
            app.port = port;
            self.registry.upsert(app.clone()).await.ok();
        }

        let proc_arc = self.get_or_create_process(slug, app.port).await;
        {
            let mut proc = proc_arc.lock().await;
            if matches!(proc.state, AppState::Running | AppState::Starting) {
                info!(app_slug = slug, "start: already running");
                return Ok(());
            }
            proc.stop_requested = false;
            proc.state = AppState::Starting;
        }
        self.update_app_state(slug, AppState::Starting).await;

        // If a stale unit is lingering (e.g. after a crash while we were offline), reset it.
        let _ = run_systemctl(&["reset-failed", &unit_name(slug)]).await;

        if !port_is_free(app.port).await && !unit_is_active(slug).await {
            warn!(app_slug = slug, port = app.port, "start: port not free (foreign process)");
            {
                let mut proc = proc_arc.lock().await;
                proc.state = AppState::Crashed;
            }
            self.update_app_state(slug, AppState::Crashed).await;
            return Err(anyhow!("port {} is already in use", app.port));
        }

        if let Err(e) = systemd_run_app(&app).await {
            error!(app_slug = %slug, error = %e, "systemd-run failed");
            {
                let mut proc = proc_arc.lock().await;
                proc.state = AppState::Crashed;
            }
            self.update_app_state(slug, AppState::Crashed).await;
            return Err(e);
        }

        self.attach_watcher(app, proc_arc).await;
        Ok(())
    }

    /// Stop an app: `systemctl stop` the transient unit, puis attend que l'unité
    /// soit réellement `inactive` (avec fallback SIGKILL si le process traîne).
    /// Retour synchrone : à la fin de l'await, l'unité est garantie déchargée
    /// — un `systemd-run` immédiat suivant cet appel ne se fera pas refuser.
    pub async fn stop(&self, slug: &str) -> Result<()> {
        let proc_arc = match self.processes.read().await.get(slug).cloned() {
            Some(p) => p,
            None => {
                info!(app_slug = slug, "stop: not supervised");
                return Ok(());
            }
        };

        let (watcher, health) = {
            let mut proc = proc_arc.lock().await;
            proc.stop_requested = true;
            proc.state = AppState::Stopping;
            (proc.watcher.take(), proc.health.take())
        };
        self.update_app_state(slug, AppState::Stopping).await;

        if let Some(h) = health {
            h.abort();
        }
        if let Some(w) = watcher {
            w.abort();
        }

        let unit = unit_name(slug);

        // 1. SIGTERM via `systemctl stop`. La commande retourne quand systemd
        //    accepte le job, pas quand le process est mort. STOP_TIMEOUT borne
        //    la commande systemctl elle-même contre un blocage pathologique.
        info!(app_slug = slug, unit = %unit, "stop: SIGTERM");
        match tokio::time::timeout(STOP_TIMEOUT, run_systemctl(&["stop", &unit])).await {
            Err(_) => warn!(app_slug = slug, unit = %unit, "stop: systemctl stop hung past STOP_TIMEOUT, will keep waiting on unit state"),
            Ok(Err(e)) => warn!(app_slug = slug, unit = %unit, error = %e, "stop: systemctl stop failed, will keep waiting on unit state"),
            Ok(Ok(_)) => {}
        }

        // 2. Attendre la désactivation effective (ActiveState=inactive|failed).
        if !wait_unit_inactive(slug, STOP_GRACE).await {
            warn!(
                app_slug = slug,
                unit = %unit,
                grace_secs = STOP_GRACE.as_secs(),
                "stop: graceful shutdown timed out, sending SIGKILL"
            );
            let _ = run_systemctl(&[
                "kill",
                "--signal=SIGKILL",
                "--kill-whom=all",
                &unit,
            ])
            .await;
            if !wait_unit_inactive(slug, STOP_KILL_GRACE).await {
                error!(app_slug = slug, unit = %unit, "stop: unit still active after SIGKILL");
                return Err(anyhow!("unit {} refused to stop after SIGKILL", unit));
            }
            info!(app_slug = slug, unit = %unit, "stop: SIGKILL succeeded");
        } else {
            info!(app_slug = slug, unit = %unit, "stop: unit inactive");
        }

        // 3. reset-failed pour cleaner si l'unité est en `failed` (SIGKILL ou
        //    crash final). Sans ça, certaines opérations systemd ultérieures
        //    pourraient considérer le slot encore occupé.
        let _ = run_systemctl(&["reset-failed", &unit]).await;

        {
            let mut proc = proc_arc.lock().await;
            proc.state = AppState::Stopped;
            proc.pid = None;
            proc.started_at = None;
        }
        self.update_app_state(slug, AppState::Stopped).await;
        Ok(())
    }

    /// Restart an app (stop then start). `stop()` est synchrone — il garantit
    /// que l'unité systemd est `inactive`/déchargée avant retour, donc on peut
    /// enchaîner directement sur `start()` sans risque de collision.
    pub async fn restart(&self, slug: &str) -> Result<()> {
        info!(app_slug = slug, "restart");
        self.stop(slug).await?;
        self.start(slug).await
    }

    /// Status snapshot for one supervised app.
    pub async fn status(&self, slug: &str) -> Option<ProcessStatus> {
        let processes = self.processes.read().await;
        let proc_arc = processes.get(slug)?.clone();
        drop(processes);
        let proc = proc_arc.lock().await;
        Some(proc.status())
    }

    /// For each app with persisted state `Running`, either adopt the existing systemd unit
    /// (if still active) or start it fresh. Called once at orchestrator boot.
    pub async fn start_all_running(&self) -> Result<()> {
        let apps = self.registry.list().await;
        for app in apps {
            if !matches!(app.state, AppState::Running | AppState::Starting) {
                continue;
            }
            if unit_is_active(&app.slug).await {
                info!(app_slug = %app.slug, "adopt: unit still active, attaching watcher");
                let proc_arc = self.get_or_create_process(&app.slug, app.port).await;
                {
                    let mut proc = proc_arc.lock().await;
                    proc.state = AppState::Running;
                    proc.started_at = Some(Instant::now());
                    proc.last_start = Some(Instant::now());
                    proc.pid = unit_main_pid(&app.slug).await;
                }
                self.update_app_state(&app.slug, AppState::Running).await;
                let app_clone = app.clone();
                self.attach_watcher(app_clone, proc_arc).await;
            } else if let Err(e) = self.start(&app.slug).await {
                error!(app_slug = %app.slug, error = %e, "start_all_running failed");
            }
        }
        Ok(())
    }

    /// Detach from supervised apps without killing them. Called at orchestrator shutdown
    /// so that a redeploy of hr-orchestrator does not interrupt running apps.
    pub async fn detach_all(&self) {
        let slugs: Vec<String> = self.processes.read().await.keys().cloned().collect();
        for slug in slugs {
            let proc_arc = match self.processes.read().await.get(&slug).cloned() {
                Some(p) => p,
                None => continue,
            };
            let (watcher, health) = {
                let mut proc = proc_arc.lock().await;
                (proc.watcher.take(), proc.health.take())
            };
            if let Some(h) = health {
                h.abort();
            }
            if let Some(w) = watcher {
                w.abort();
            }
            info!(app_slug = %slug, "detach: leaving systemd unit running");
        }
    }

    async fn get_or_create_process(&self, slug: &str, port: u16) -> Arc<Mutex<SupervisedProcess>> {
        {
            let processes = self.processes.read().await;
            if let Some(p) = processes.get(slug) {
                return p.clone();
            }
        }
        let mut processes = self.processes.write().await;
        processes
            .entry(slug.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(SupervisedProcess::new(port))))
            .clone()
    }

    async fn update_app_state(&self, slug: &str, state: AppState) {
        if let Some(mut app) = self.registry.get(slug).await {
            if app.state != state {
                app.state = state;
                if let Err(e) = self.registry.upsert(app.clone()).await {
                    warn!(app_slug = slug, error = %e, "update_app_state persist failed");
                }
                let proc_status = {
                    let procs = self.processes.read().await;
                    procs
                        .get(slug)
                        .and_then(|p| p.try_lock().ok().map(|p| p.status()))
                };
                let event = hr_common::events::AppStateEvent {
                    slug: slug.to_string(),
                    state: format!("{:?}", state).to_lowercase(),
                    pid: proc_status.as_ref().and_then(|s| s.pid),
                    port: app.port,
                    uptime_secs: proc_status.as_ref().map(|s| s.uptime_secs).unwrap_or(0),
                    restart_count: proc_status.as_ref().map(|s| s.restart_count).unwrap_or(0),
                };
                let _ = self.app_state_tx.send(event);
            }
        }
    }

    async fn attach_watcher(&self, app: Application, proc_arc: Arc<Mutex<SupervisedProcess>>) {
        let supervisor = self.clone();
        let slug = app.slug.clone();
        let port = app.port;
        let health_path = app.health_path.clone();
        let handle = tokio::spawn(async move {
            supervisor.watcher_loop(app, proc_arc).await;
        });
        if let Some(p) = self.processes.read().await.get(&slug) {
            let mut proc = p.lock().await;
            if let Some(old) = proc.watcher.replace(handle) {
                old.abort();
            }
            if proc.health.is_none() {
                proc.health = Some(self.spawn_health_loop(slug.clone(), port, health_path));
            }
        }
    }

    async fn watcher_loop(&self, app: Application, proc_arc: Arc<Mutex<SupervisedProcess>>) {
        let slug = app.slug.clone();
        // Wait up to 5s for the unit to become active after systemd-run.
        for _ in 0..25 {
            if unit_is_active(&slug).await {
                let pid = unit_main_pid(&slug).await;
                let mut proc = proc_arc.lock().await;
                proc.pid = pid;
                proc.state = AppState::Running;
                proc.started_at = Some(Instant::now());
                proc.last_start = Some(Instant::now());
                drop(proc);
                self.update_app_state(&slug, AppState::Running).await;
                break;
            }
            sleep(Duration::from_millis(200)).await;
        }

        loop {
            sleep(WATCH_INTERVAL).await;

            let stop_requested = {
                let proc = proc_arc.lock().await;
                proc.stop_requested
            };
            if stop_requested {
                debug!(app_slug = %slug, "watcher: stop requested, exiting");
                return;
            }

            if unit_is_active(&slug).await {
                // Refresh PID in case it changed (shouldn't for transient simple service).
                let pid = unit_main_pid(&slug).await;
                let mut proc = proc_arc.lock().await;
                if proc.pid != pid {
                    proc.pid = pid;
                }
                continue;
            }

            // Unit not active: crashed, stopped externally, ou en cours de stop volontaire.
            let result = unit_result(&slug).await.unwrap_or_default();

            // Re-check stop_requested ici : `stop()` peut avoir flippé le flag
            // pendant qu'on était dans `unit_is_active().await`. Si c'est le cas,
            // on est en train d'observer un shutdown volontaire — ne pas écraser
            // l'état `Stopping` posé par stop() avec `Crashed`.
            let stop_requested_now = {
                let proc = proc_arc.lock().await;
                proc.stop_requested
            };
            if stop_requested_now {
                debug!(
                    app_slug = %slug,
                    result = %result,
                    "watcher: unit no longer active, but stop requested — exiting without Crashed"
                );
                return;
            }

            warn!(app_slug = %slug, result = %result, "watcher: unit no longer active");

            {
                let mut proc = proc_arc.lock().await;
                if let Some(h) = proc.health.take() {
                    h.abort();
                }
                proc.pid = None;
                proc.started_at = None;
                proc.state = AppState::Crashed;
            }
            self.update_app_state(&slug, AppState::Crashed).await;

            let _ = run_systemctl(&["reset-failed", &unit_name(&slug)]).await;

            if !self.bump_restart_and_should_continue(&proc_arc).await {
                error!(app_slug = %slug, "restart limit exceeded, giving up");
                return;
            }

            let backoff = self.current_backoff(&proc_arc).await;
            warn!(app_slug = %slug, backoff_ms = backoff.as_millis() as u64, "respawning after backoff");
            sleep(backoff).await;

            {
                let mut proc = proc_arc.lock().await;
                if proc.stop_requested {
                    return;
                }
                proc.state = AppState::Starting;
            }
            self.update_app_state(&slug, AppState::Starting).await;

            if let Err(e) = systemd_run_app(&app).await {
                error!(app_slug = %slug, error = %e, "respawn systemd-run failed");
                let mut proc = proc_arc.lock().await;
                proc.state = AppState::Crashed;
                drop(proc);
                self.update_app_state(&slug, AppState::Crashed).await;
                continue;
            }

            {
                let mut proc = proc_arc.lock().await;
                if proc.health.is_none() {
                    proc.health = Some(self.spawn_health_loop(
                        slug.clone(),
                        app.port,
                        app.health_path.clone(),
                    ));
                }
            }
        }
    }

    async fn bump_restart_and_should_continue(
        &self,
        proc_arc: &Arc<Mutex<SupervisedProcess>>,
    ) -> bool {
        let mut proc = proc_arc.lock().await;
        if let Some(last) = proc.last_start {
            if last.elapsed() >= RESTART_RESET_AFTER {
                proc.next_backoff = Duration::from_secs(1);
            }
        }
        proc.restart_count += 1;
        let now = Instant::now();
        proc.restart_history
            .retain(|t| now.duration_since(*t) < RESTART_WINDOW);
        proc.restart_history.push(now);
        if proc.restart_history.len() as u32 > MAX_RESTARTS_PER_MIN {
            proc.state = AppState::Crashed;
            return false;
        }
        true
    }

    async fn current_backoff(&self, proc_arc: &Arc<Mutex<SupervisedProcess>>) -> Duration {
        let mut proc = proc_arc.lock().await;
        let current = proc.next_backoff;
        let next_secs = (current.as_secs() * 2).min(30).max(1);
        proc.next_backoff = Duration::from_secs(next_secs);
        current
    }

    fn spawn_health_loop(&self, slug: String, port: u16, path: String) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut consecutive_failures = 0u32;
            loop {
                sleep(HEALTH_INTERVAL).await;
                let ok = http_health_check(port, &path).await;
                if ok {
                    if consecutive_failures > 0 {
                        info!(app_slug = %slug, port, "health recovered");
                    }
                    consecutive_failures = 0;
                } else {
                    consecutive_failures += 1;
                    if consecutive_failures >= 3 {
                        warn!(
                            app_slug = %slug,
                            port,
                            failures = consecutive_failures,
                            "health check failing"
                        );
                    }
                }
            }
        })
    }
}

async fn systemd_run_app(app: &Application) -> Result<()> {
    let src_dir = app.src_dir();
    let env_file = app.env_file();
    let unit = unit_name(&app.slug);

    let mut cmd = Command::new("systemd-run");
    cmd.arg(format!("--unit={unit}"));
    cmd.arg(format!("--slice={SLICE}"));
    cmd.arg("--collect");
    cmd.arg("--quiet");
    cmd.arg("--no-block");
    cmd.arg(format!("--working-directory={}", src_dir.display()));
    cmd.arg(format!("--description=HomeRoute app {}", app.slug));
    cmd.arg(format!("--setenv=PORT={}", app.port));
    cmd.arg(format!(
        "--setenv=DATABASE_PATH={}",
        app.db_path().display()
    ));
    cmd.arg(format!("--setenv=DB_PATH={}", app.db_path().display()));
    for (k, v) in &app.env_vars {
        cmd.arg(format!("--setenv={k}={v}"));
    }
    if env_file.exists() {
        match load_env_file(&env_file).await {
            Ok(vars) => {
                for (k, v) in vars {
                    cmd.arg(format!("--setenv={k}={v}"));
                }
            }
            Err(e) => {
                warn!(app_slug = %app.slug, path = %env_file.display(), error = %e, "env file load failed");
            }
        }
    }

    cmd.arg("--");
    cmd.arg("/bin/bash");
    cmd.arg("-c");
    cmd.arg(&app.run_command);

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd
        .output()
        .await
        .with_context(|| format!("launching systemd-run for {}", app.slug))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "systemd-run exited {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    info!(app_slug = %app.slug, unit, "systemd-run ok");
    Ok(())
}

async fn run_systemctl(args: &[&str]) -> Result<std::process::Output> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .await
        .with_context(|| format!("systemctl {:?}", args))?;
    Ok(output)
}

async fn systemctl_show(slug: &str, prop: &str) -> Option<String> {
    let out = Command::new("systemctl")
        .args([
            "show",
            &unit_name(slug),
            "-p",
            prop,
            "--value",
            "--no-pager",
        ])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if val.is_empty() { None } else { Some(val) }
}

async fn unit_is_active(slug: &str) -> bool {
    matches!(
        systemctl_show(slug, "ActiveState").await.as_deref(),
        Some("active") | Some("activating") | Some("reloading")
    )
}

/// Attend que l'unité systemd atteigne `ActiveState=inactive|failed` (ou disparaisse).
///
/// `systemctl stop` retourne dès que systemd accepte la requête, pas quand le
/// process supervisé a effectivement terminé son shutdown gracieux. Tant que
/// l'unité reste `deactivating`, `systemd-run --unit=...` refuse de la recréer
/// (« Unit hr-app-xxx.service was already loaded or has a fragment file »).
/// On poll donc jusqu'à confirmation que l'unité est libre.
///
/// Avec `--collect`, l'unité transient est automatiquement déchargée juste après
/// le passage à `inactive`/`failed` — donc dès qu'on observe l'un de ces états,
/// la prochaine création de la même unité fonctionne.
///
/// Retourne `true` si la fenêtre s'est terminée à temps, `false` sur timeout.
async fn wait_unit_inactive(slug: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let state = systemctl_show(slug, "ActiveState").await;
        match state.as_deref() {
            Some("inactive") | Some("failed") | None => return true,
            _ => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(STOP_POLL_INTERVAL).await;
    }
}

async fn unit_main_pid(slug: &str) -> Option<u32> {
    let v = systemctl_show(slug, "MainPID").await?;
    let pid: u32 = v.parse().ok()?;
    if pid == 0 { None } else { Some(pid) }
}

async fn unit_result(slug: &str) -> Option<String> {
    systemctl_show(slug, "Result").await
}

async fn load_env_file(path: &PathBuf) -> Result<Vec<(String, String)>> {
    let bytes = tokio::fs::read(path).await?;
    let text = String::from_utf8_lossy(&bytes);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim().to_string();
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !k.is_empty() {
                out.push((k, v));
            }
        }
    }
    Ok(out)
}

async fn port_is_free(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).await.is_err()
}

async fn http_health_check(port: u16, path: &str) -> bool {
    let connect = TcpStream::connect(("127.0.0.1", port));
    let stream_res = tokio::time::timeout(HEALTH_TIMEOUT, connect).await;
    let mut stream = match stream_res {
        Ok(Ok(s)) => s,
        _ => return false,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = [0u8; 256];
    let read = tokio::time::timeout(HEALTH_TIMEOUT, stream.read(&mut buf)).await;
    let n = match read {
        Ok(Ok(n)) if n > 0 => n,
        _ => return false,
    };
    let resp = String::from_utf8_lossy(&buf[..n]);
    let line = match resp.lines().next() {
        Some(l) => l,
        None => return false,
    };
    if !line.starts_with("HTTP/") {
        return false;
    }
    line.split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .map(|c| (200..400).contains(&c))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_name_format() {
        assert_eq!(unit_name("foo"), "hr-app-foo.service");
    }
}
