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
    let mut app_todos_rx = state.events.app_todos.subscribe();

    loop {
        tokio::select! {
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
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            result = app_todos_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = json!({ "type": "app:todos", "data": event });
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(topic = "app:todos", dropped = n, "ws subscriber lagged");
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
