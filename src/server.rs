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
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};

/// How a session's connection should be torn down when triggered via [`ServerControl`].
#[derive(Debug, Clone)]
pub enum CloseMode {
    /// Send a WebSocket close frame with the given code and reason.
    Frame { code: u16, reason: String },
    /// Send a well-formed close handshake with no payload at all (`Message::Close(None)`).
    /// Distinct from [`CloseMode::Abrupt`]: the handshake still completes, it just carries
    /// no code/reason (many clients surface this as close code 1005, "no status received").
    EmptyFrame,
    /// Drop the connection without performing a WebSocket close handshake, simulating an
    /// abrupt disconnect (e.g. a crash or network failure) rather than a clean shutdown.
    /// Many clients surface this as close code 1006, "abnormal closure".
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
    /// Raw text of every client message received so far, in arrival order. Populated
    /// regardless of whether the message parses as a well-formed [`ClientMessage`], so
    /// tests can assert on a marker string without depending on protocol details.
    received: Mutex<Vec<String>>,
    /// The active connection's event sender, the same one [`Session`] uses internally to
    /// queue outbound messages. Populated once a connection is accepted, so
    /// [`ServerControl::send_message`] can queue a message onto the same path a real
    /// session response would take.
    active_event_tx: watch::Sender<Option<mpsc::Sender<SessionEvent>>>,
    active_event_rx: watch::Receiver<Option<mpsc::Sender<SessionEvent>>>,
}

/// A handle for controlling the active session on a server started with
/// [`run_server_with_control`]. The close code/reason (or abrupt-drop behavior) is fixed
/// up front via [`CloseMode`]; [`ServerControl::trigger_close`] just fires it.
#[derive(Clone)]
pub struct ServerControl {
    inner: Arc<SessionControl>,
    /// Reports the address the server actually bound to, so callers can use
    /// `127.0.0.1:0` in [`ServerConfig::listen`] and still know where to connect.
    /// Populated once, right after the listener binds - or with `Err` if binding failed,
    /// so [`ServerControl::local_addr`] can report that instead of hanging forever (the
    /// sender lives as long as [`run_server_inner`]'s task does, so it never drops on its
    /// own to signal that no value is coming).
    local_addr_rx: watch::Receiver<Option<Result<SocketAddr, String>>>,
}

impl ServerControl {
    /// Tear down the active session's connection using the [`CloseMode`] configured in
    /// [`run_server_with_control`].
    pub fn trigger_close(&self) {
        self.inner.token.cancel();
    }

    /// Returns the raw text of every client message received by the active session so
    /// far, in arrival order. Useful for asserting that a particular message actually
    /// reached the server, e.g. one sent during a shutdown grace period.
    pub fn received_messages(&self) -> Vec<String> {
        self.inner.received.lock().unwrap().clone()
    }

    /// Resolves to the address the server actually bound to. Lets callers configure
    /// [`ServerConfig::listen`] as `127.0.0.1:0` (an OS-assigned ephemeral port, safe for
    /// concurrent tests) and still learn which port to connect to.
    ///
    /// # Panics
    ///
    /// Panics if the server failed to bind its listening socket.
    pub async fn local_addr(&self) -> SocketAddr {
        let mut rx = self.local_addr_rx.clone();
        loop {
            if let Some(result) = rx.borrow().clone() {
                return result.expect("server failed to bind its listening socket");
            }
            rx.changed()
                .await
                .expect("server task dropped without reporting its bound address");
        }
    }

    /// Sends `msg` to the active session's client right away, on the same delivery path
    /// [`Session`] uses for its own responses - so it arrives as an ordinary server
    /// message (e.g. a [`crate::protocol::ServerContentMessage`]) rather than anything
    /// distinguishable as test-injected. Useful for asserting things like whether a
    /// message sent during a specific window (e.g. a shutdown grace period) actually
    /// reaches the client.
    ///
    /// Waits for a connection to be active if none has been accepted yet.
    pub async fn send_message<T: serde::Serialize>(
        &self,
        msg: &T,
    ) -> Result<(), SendMessageError> {
        let json = serde_json::to_string(msg).map_err(SendMessageError::Serialize)?;

        let mut rx = self.inner.active_event_rx.clone();
        let tx = loop {
            if let Some(tx) = rx.borrow().clone() {
                break tx;
            }
            rx.changed()
                .await
                .map_err(|_| SendMessageError::NoActiveSession)?;
        };

        tx.send(SessionEvent::SendMessage(json))
            .await
            .map_err(|_| SendMessageError::NoActiveSession)
    }
}

/// Error returned by [`ServerControl::send_message`].
#[derive(Debug)]
pub enum SendMessageError {
    /// The message could not be serialized to JSON.
    Serialize(serde_json::Error),
    /// There is no active session's connection left to deliver the message to (it ended,
    /// or its event channel closed).
    NoActiveSession,
}

impl std::fmt::Display for SendMessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendMessageError::Serialize(e) => write!(f, "failed to serialize message: {e}"),
            SendMessageError::NoActiveSession => {
                write!(f, "no active session to deliver the message to")
            }
        }
    }
}

impl std::error::Error for SendMessageError {}

pub async fn run_server(config: ServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    run_server_inner(config, None, None).await
}

/// Starts the server like [`run_server`], but also returns a `watch::Receiver` that
/// resolves to the address the server actually bound to - useful for configuring
/// [`ServerConfig::listen`] as `127.0.0.1:0` and still learning which port to connect to,
/// without needing the session-control machinery of [`run_server_with_control`].
pub fn run_server_with_addr(
    config: ServerConfig,
) -> (
    impl Future<Output = Result<(), Box<dyn std::error::Error>>>,
    watch::Receiver<Option<Result<SocketAddr, String>>>,
) {
    let (local_addr_tx, local_addr_rx) = watch::channel(None);
    (
        run_server_inner(config, Some(local_addr_tx), None),
        local_addr_rx,
    )
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
    let (local_addr_tx, local_addr_rx) = watch::channel(None);
    let (active_event_tx, active_event_rx) = watch::channel(None);
    let inner = Arc::new(SessionControl {
        token: CancellationToken::new(),
        mode,
        received: Mutex::new(Vec::new()),
        active_event_tx,
        active_event_rx,
    });
    let control = ServerControl {
        inner: inner.clone(),
        local_addr_rx,
    };
    (
        run_server_inner(config, Some(local_addr_tx), Some(inner)),
        control,
    )
}

async fn run_server_inner(
    config: ServerConfig,
    local_addr_tx: Option<watch::Sender<Option<Result<SocketAddr, String>>>>,
    control: Option<Arc<SessionControl>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listen_addr = config.listen;

    match control.as_ref().map(|c| &c.mode) {
        Some(CloseMode::Frame { code, reason }) => info!(
            close_code = code,
            close_reason = %reason,
            "Session control enabled: sessions can be closed with a custom close frame"
        ),
        Some(CloseMode::EmptyFrame) => {
            info!("Session control enabled: sessions can be closed with an empty close frame")
        }
        Some(CloseMode::Abrupt) => {
            info!("Session control enabled: sessions can be closed abruptly (no close handshake)")
        }
        None => {}
    }

    let listener = match tokio::net::TcpListener::bind(listen_addr).await {
        Ok(listener) => listener,
        Err(e) => {
            if let Some(tx) = &local_addr_tx {
                let _ = tx.send(Some(Err(e.to_string())));
            }
            return Err(e.into());
        }
    };
    let bound_addr = listener.local_addr()?;
    info!("Mock Gemini Live server listening on {}", bound_addr);
    if let Some(tx) = &local_addr_tx {
        let _ = tx.send(Some(Ok(bound_addr)));
    }

    let app_state = Arc::new(AppState { config, control });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/", get(ws_handler))
        .route("/health", get(health_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(app_state);

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
    if let Some(control) = &control {
        let _ = control.active_event_tx.send(Some(event_tx.clone()));
    }
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
                    Some(SessionControl { mode: CloseMode::EmptyFrame, .. }) => {
                        info!(session_id = %session_id, "Closing connection via control trigger (empty close frame)");
                        let _ = ws_sink.send(Message::Close(None)).await;
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
                        if let Some(control) = &control {
                            control.received.lock().unwrap().push(text.to_string());
                        }
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
                            if let Some(control) = &control {
                                control.received.lock().unwrap().push(text.clone());
                            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::SinkExt;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as ClientWsMessage;

    #[tokio::test]
    async fn received_messages_captures_raw_client_text() {
        let config = ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            ..Default::default()
        };
        let (server, control) = run_server_with_control(config, CloseMode::Abrupt);
        let _server_task = tokio::spawn(async move {
            let _ = server.await;
        });

        let addr = control.local_addr().await;

        assert!(control.received_messages().is_empty());

        let (mut ws_stream, _) = connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("failed to connect to mock server");

        let marker =
            r#"{"clientContent":{"turns":[],"turnComplete":false},"marker":"test-message-123"}"#;
        ws_stream
            .send(ClientWsMessage::Text(marker.into()))
            .await
            .expect("failed to send message");

        // Give the server a moment to process the message.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let received = control.received_messages();
        assert_eq!(received.len(), 1);
        assert!(received[0].contains("test-message-123"));
    }

    #[tokio::test]
    async fn send_message_delivers_to_connected_client() {
        let config = ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            ..Default::default()
        };
        let (server, control) = run_server_with_control(config, CloseMode::Abrupt);
        let _server_task = tokio::spawn(async move {
            let _ = server.await;
        });

        let addr = control.local_addr().await;
        let (mut ws_stream, _) = connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("failed to connect to mock server");

        let msg = crate::protocol::ServerContentMessage {
            server_content: crate::protocol::ServerContent {
                model_turn: None,
                turn_complete: Some(true),
                interrupted: None,
            },
        };
        control
            .send_message(&msg)
            .await
            .expect("failed to send message via control");

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), ws_stream.next())
            .await
            .expect("timed out waiting for message")
            .expect("stream ended")
            .expect("websocket error");

        match received {
            ClientWsMessage::Text(text) => {
                assert!(text.contains("turnComplete"));
            }
            other => panic!("expected a text message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_message_before_any_connection_waits_then_delivers() {
        let config = ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            ..Default::default()
        };
        let (server, control) = run_server_with_control(config, CloseMode::Abrupt);
        let _server_task = tokio::spawn(async move {
            let _ = server.await;
        });

        let addr = control.local_addr().await;
        let control_for_send = control.clone();
        let send_task = tokio::spawn(async move {
            let msg = crate::protocol::ServerContentMessage {
                server_content: crate::protocol::ServerContent {
                    model_turn: None,
                    turn_complete: Some(true),
                    interrupted: None,
                },
            };
            control_for_send.send_message(&msg).await
        });

        // Give send_message a moment to start waiting on a connection before one exists.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (mut ws_stream, _) = connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("failed to connect to mock server");

        send_task
            .await
            .expect("send task panicked")
            .expect("failed to send message via control");

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), ws_stream.next())
            .await
            .expect("timed out waiting for message")
            .expect("stream ended")
            .expect("websocket error");

        match received {
            ClientWsMessage::Text(text) => {
                assert!(text.contains("turnComplete"));
            }
            other => panic!("expected a text message, got {other:?}"),
        }
    }

    #[tokio::test]
    #[should_panic(expected = "server failed to bind its listening socket")]
    async fn local_addr_reports_bind_failure_instead_of_hanging() {
        // Occupy a port so the mock server's own bind attempt fails.
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = blocker.local_addr().unwrap();

        let config = ServerConfig {
            listen: addr,
            ..Default::default()
        };
        let (server, control) = run_server_with_control(config, CloseMode::Abrupt);
        let _server_task = tokio::spawn(async move {
            let _ = server.await;
        });

        // Bound so a regression back to "hangs forever" fails loudly instead of wedging
        // the test suite - this panics with a different message, which fails the
        // `should_panic(expected = ...)` match above.
        match tokio::time::timeout(std::time::Duration::from_secs(5), control.local_addr()).await
        {
            Ok(addr) => panic!("expected local_addr() to panic on bind failure, got {addr}"),
            Err(_) => panic!("local_addr() hung instead of reporting the bind failure"),
        }
    }

    #[tokio::test]
    async fn run_server_with_addr_reports_bound_address() {
        let config = ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            ..Default::default()
        };
        let (server, mut addr_rx) = run_server_with_addr(config);
        let _server_task = tokio::spawn(async move {
            let _ = server.await;
        });

        let addr = loop {
            if let Some(result) = addr_rx.borrow().clone() {
                break result.expect("server failed to bind its listening socket");
            }
            addr_rx
                .changed()
                .await
                .expect("server task dropped without reporting its bound address");
        };

        connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("failed to connect to mock server");
    }
}
