use crate::ServerConfig;
use crate::protocol::ClientMessage;
use crate::session::{Session, SessionEvent};
use axum::{
    Router,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};

/// How a session's connection should be torn down when triggered via [`ServerControl`].
#[derive(Debug, Clone)]
pub enum CloseMode {
    /// Send a WebSocket close frame with the given code and reason.
    Frame { code: u16, reason: String },
    /// Drop the connection without performing a WebSocket close handshake, simulating an
    /// abrupt disconnect (e.g. a crash or network failure) rather than a clean shutdown.
    Abrupt,
}

struct AppState {
    config: ServerConfig,
    control: Option<Arc<SessionControl>>,
}

/// Shared state that lets a [`ServerControl`] handle reach into the currently active
/// session's connection loop. Only meaningful for one connection at a time, which is
/// sufficient for test usage.
struct SessionControl {
    token: CancellationToken,
    mode: CloseMode,
}

/// A handle for controlling the active session on a server started with
/// [`run_server_with_control`]. The close code/reason (or abrupt-drop behavior) is fixed
/// up front via [`CloseMode`]; [`ServerControl::trigger_close`] just fires it.
#[derive(Clone)]
pub struct ServerControl {
    inner: Arc<SessionControl>,
}

impl ServerControl {
    /// Tear down the active session's connection using the [`CloseMode`] configured in
    /// [`run_server_with_control`].
    pub fn trigger_close(&self) {
        self.inner.token.cancel();
    }
}

pub async fn run_server(config: ServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    run_server_inner(config, None).await
}

/// Starts the server like [`run_server`], but also returns a [`ServerControl`] handle that
/// test code can use to close the active session's connection on demand, using the given
/// [`CloseMode`].
pub fn run_server_with_control(
    config: ServerConfig,
    mode: CloseMode,
) -> (
    impl Future<Output = Result<(), Box<dyn std::error::Error>>>,
    ServerControl,
) {
    let inner = Arc::new(SessionControl {
        token: CancellationToken::new(),
        mode,
    });
    let control = ServerControl {
        inner: inner.clone(),
    };
    (run_server_inner(config, Some(inner)), control)
}

async fn run_server_inner(
    config: ServerConfig,
    control: Option<Arc<SessionControl>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listen_addr = config.listen;

    match control.as_ref().map(|c| &c.mode) {
        Some(CloseMode::Frame { code, reason }) => info!(
            close_code = code,
            close_reason = %reason,
            "Session control enabled: sessions can be closed with a custom close frame"
        ),
        Some(CloseMode::Abrupt) => {
            info!("Session control enabled: sessions can be closed abruptly (no close handshake)")
        }
        None => {}
    }

    let app_state = Arc::new(AppState { config, control });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/", get(ws_handler))
        .route("/health", get(health_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    info!("Mock Gemini Live server listening on {}", listen_addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

async fn health_handler() -> impl IntoResponse {
    "OK"
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(app_state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    info!(client_addr = %addr, "WebSocket upgrade request");
    let config = app_state.config.clone();
    let control = app_state.control.clone();
    ws.on_upgrade(move |socket| handle_connection(socket, addr, config, control))
}

async fn handle_connection(
    socket: WebSocket,
    addr: SocketAddr,
    config: ServerConfig,
    control: Option<Arc<SessionControl>>,
) {
    info!(client_addr = %addr, "WebSocket connection established");

    let (mut ws_sink, mut ws_stream) = socket.split();

    // Create cancellation token for this connection. When external control is enabled,
    // this is a child of the control token, so triggering the control cancels this too
    // while still letting us tell the two cases apart (see below).
    let cancellation_token = match &control {
        Some(control) => control.token.child_token(),
        None => CancellationToken::new(),
    };

    // Create session
    let session_id = uuid::Uuid::new_v4().to_string();
    let (event_tx, mut event_rx) = mpsc::channel::<SessionEvent>(32);
    let mut session = Session::new(
        session_id.clone(),
        config,
        event_tx,
        cancellation_token.clone(),
    );

    info!(
        session_id = %session_id,
        client_addr = %addr,
        "Session created"
    );

    loop {
        tokio::select! {
            // Handle cancellation
            _ = cancellation_token.cancelled() => {
                // Distinguish an externally-triggered close (the parent control token was
                // cancelled) from an internal one (e.g. client disconnect, error) - only the
                // former should apply the configured CloseMode.
                match control.as_deref().filter(|c| c.token.is_cancelled()) {
                    Some(SessionControl { mode: CloseMode::Frame { code, reason }, .. }) => {
                        info!(session_id = %session_id, code = code, reason = %reason, "Closing connection via control trigger");
                        let _ = ws_sink
                            .send(Message::Close(Some(CloseFrame {
                                code: *code,
                                reason: reason.clone().into(),
                            })))
                            .await;
                    }
                    Some(SessionControl { mode: CloseMode::Abrupt, .. }) => {
                        info!(session_id = %session_id, "Dropping connection via control trigger (abrupt)");
                        // Intentionally no close frame - just stop driving the socket.
                    }
                    None => {
                        info!(session_id = %session_id, "Session cancelled");
                        let _ = ws_sink.close().await;
                    }
                }
                break;
            }

            // Handle incoming WebSocket messages
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!(
                            session_id = %session_id,
                            message_len = text.len(),
                            "Received text message"
                        );
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(client_msg) => {
                                let msg_type = client_msg.message_type();
                                info!(
                                    session_id = %session_id,
                                    message_type = %msg_type,
                                    "Processing client message"
                                );
                                session.handle_message(client_msg).await;
                            }
                            Err(e) => {
                                warn!(
                                    session_id = %session_id,
                                    error = %e,
                                    "Failed to parse message"
                                );
                                debug!(raw_message = %text, "Raw unparseable message");
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        debug!(
                            session_id = %session_id,
                            data_len = data.len(),
                            "Received binary message"
                        );
                        // Try to parse binary as JSON (some clients send as binary)
                        if let Ok(text) = String::from_utf8(data.to_vec()) {
                            if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                                let msg_type = client_msg.message_type();
                                info!(
                                    session_id = %session_id,
                                    message_type = %msg_type,
                                    "Processing client message (binary)"
                                );
                                session.handle_message(client_msg).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!(
                            session_id = %session_id,
                            client_addr = %addr,
                            "Client sent close frame"
                        );
                        cancellation_token.cancel();
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        debug!(session_id = %session_id, "Received ping");
                        if let Err(e) = ws_sink.send(Message::Pong(data)).await {
                            warn!(session_id = %session_id, error = %e, "Failed to send pong");
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        debug!(session_id = %session_id, "Received pong");
                    }
                    Some(Err(e)) => {
                        error!(session_id = %session_id, error = %e, "WebSocket error");
                        cancellation_token.cancel();
                        break;
                    }
                    None => {
                        info!(session_id = %session_id, "Connection closed");
                        cancellation_token.cancel();
                        break;
                    }
                }
            }

            // Handle outgoing session events
            event = event_rx.recv() => {
                match event {
                    Some(SessionEvent::SendMessage(json)) => {
                        debug!(
                            session_id = %session_id,
                            message_len = json.len(),
                            "Sending message to client"
                        );
                        if let Err(e) = ws_sink.send(Message::Text(json.into())).await {
                            error!(session_id = %session_id, error = %e, "Failed to send message");
                            cancellation_token.cancel();
                            break;
                        }
                    }
                    Some(SessionEvent::Close) => {
                        info!(session_id = %session_id, "Closing connection");
                        cancellation_token.cancel();
                        let _ = ws_sink.close().await;
                        break;
                    }
                    None => {
                        // Channel closed
                        debug!(session_id = %session_id, "Event channel closed");
                        cancellation_token.cancel();
                        break;
                    }
                }
            }
        }
    }

    session.finalize();
    info!(
        session_id = %session_id,
        client_addr = %addr,
        "Session ended"
    );
}
