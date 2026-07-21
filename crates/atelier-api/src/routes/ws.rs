use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use serde_json::json;
use tokio::sync::broadcast;
use tracing::{debug, instrument, warn};

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new().route("/ws", get(ws_handler))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<ApiState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

#[instrument(skip(state, socket))]
async fn handle_socket(mut socket: WebSocket, state: ApiState) {
    debug!("ws client connected");

    // task_update has no Atelier-side publisher today (TaskStore is read-only
    // here — populated by external sync). Channel is wired for future
    // task lifecycle changes to light up TaskContext / TaskDetail automatically.
    let mut task_update_rx = state.events.task_update.subscribe();
    let mut log_rx = state.events.log_entry.subscribe();
    let mut logs_pg_rx = state.logs.subscribe();
    let mut app_state_rx = state.events.app_state.subscribe();
    let mut app_build_rx = state.events.app_build.subscribe();
    let mut source_changed_rx = state.events.source_changed.subscribe();
    let mut surveillance_rx = state.surveillance.subscribe();
    let mut transcript_rx = state.surveillance.subscribe_transcript();
    let mut sweep_rx = state.surveillance.subscribe_sweep();
    let mut backup_rx = state.backup.subscribe();
    let mut agent_rx = state.events.agent.subscribe();
    let mut agent_open_tabs_rx = state.events.agent_open_tabs.subscribe();
    let mut studio_tab_rx = state.events.studio_tab.subscribe();
    let mut homeroute_routes_rx = state.events.homeroute_routes.subscribe();
    let mut notify_rx = state.events.notify.subscribe();
    let mut issue_rx = state.events.issue.subscribe();
    let mut pilot_rx = state.pilot.subscribe();
    let mut pilot_transcript_rx = state.pilot.subscribe_transcript();
    let mut pilot_night_rx = state.pilot.subscribe_night();

    loop {
        tokio::select! {
            result = pilot_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "pilot:backlog", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "pilot:backlog", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "pilot:backlog", n).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = pilot_transcript_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "pilot:transcript", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "pilot:transcript", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "pilot:transcript", n).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = pilot_night_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "pilot:night", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "pilot:night", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "pilot:night", n).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = agent_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "agent:event", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "agent:event", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "agent:event", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = notify_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "notify:event", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "notify:event", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "notify:event", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = issue_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "issue:event", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "issue:event", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "issue:event", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = agent_open_tabs_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "agent:open-tabs", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "agent:open-tabs", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "agent:open-tabs", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = studio_tab_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "studio:tab", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "studio:tab", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "studio:tab", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = backup_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "backup:live", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "backup:live", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "backup:live", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = homeroute_routes_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "homeroute:routes", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "homeroute:routes", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "homeroute:routes", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = surveillance_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "surveillance:event", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "surveillance:event", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "surveillance:event", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = transcript_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "surveillance:transcript", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "surveillance:transcript", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "surveillance:transcript", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = sweep_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "surveillance:sweep", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "surveillance:sweep", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "surveillance:sweep", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = app_state_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "app:state", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "app:state", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "app:state", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = app_build_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "app:build", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "app:build", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "app:build", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = source_changed_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "source:changed", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "source:changed", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "source:changed", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = log_rx.recv() => {
                match result {
                    Ok(entry) => {
                        let msg = json!({ "type": "app:log", "data": entry });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "app:log", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "app:log", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = logs_pg_rx.recv() => {
                match result {
                    Ok(entry) => {
                        let msg = json!({ "type": "log:entry", "data": entry });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "log:entry", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "log:entry", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = task_update_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({
                            "type": "task:update",
                            "data": { "task": event.task, "steps": event.steps },
                        });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "task:update", dropped = n, "ws subscriber lagged");
                        if send_resync(&mut socket, "task:update", n).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    debug!("ws client disconnected");
}

/// Tell the client it missed `dropped` events on `channel` so it can refetch a
/// fresh snapshot. Without this, a dropped terminal event (`turn_done`,
/// `question`, backup `done`…) leaves the UI stuck on stale incremental state
/// with no way to notice.
async fn send_resync(
    socket: &mut WebSocket,
    channel: &str,
    dropped: u64,
) -> Result<(), axum::Error> {
    let msg = json!({ "type": "resync", "channel": channel, "dropped": dropped });
    socket.send(Message::Text(msg.to_string().into())).await
}
