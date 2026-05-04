use crate::config::Config;
use crate::protocol::ClientMessage;
use crate::session::{Session, SessionEvent};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{error, info, warn};

pub async fn run_server(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&config.listen).await?;
    info!("Mock Gemini Live server listening on {}", config.listen);

    loop {
        let (stream, addr) = listener.accept().await?;
        let config = config.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, addr, config).await {
                error!("Connection error from {}: {}", addr, e);
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    config: Config,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("New connection from {}", addr);

    let ws_stream = accept_async(stream).await?;
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // Create session
    let session_id = uuid::Uuid::new_v4().to_string();
    let (event_tx, mut event_rx) = mpsc::channel::<SessionEvent>(32);
    let mut session = Session::new(session_id.clone(), config, event_tx);

    info!("Session {} created for {}", session_id, addr);

    loop {
        tokio::select! {
            // Handle incoming WebSocket messages
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(client_msg) => {
                                session.handle_message(client_msg).await;
                            }
                            Err(e) => {
                                warn!("Failed to parse message: {e}");
                                warn!("Raw message: {text}");
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        // Try to parse binary as JSON (some clients send as binary)
                        if let Ok(text) = String::from_utf8(data.to_vec()) {
                            if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                                session.handle_message(client_msg).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("Session {} received close from {}", session_id, addr);
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if let Err(e) = ws_sink.send(Message::Pong(data)).await {
                            warn!("Failed to send pong: {e}");
                        }
                    }
                    Some(Ok(_)) => {
                        // Ignore other message types
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {e}");
                        break;
                    }
                    None => {
                        info!("Session {} connection closed", session_id);
                        break;
                    }
                }
            }

            // Handle outgoing session events
            event = event_rx.recv() => {
                match event {
                    Some(SessionEvent::SendMessage(json)) => {
                        if let Err(e) = ws_sink.send(Message::Text(json.into())).await {
                            error!("Failed to send message: {e}");
                            break;
                        }
                    }
                    Some(SessionEvent::Close) => {
                        let _ = ws_sink.close().await;
                        break;
                    }
                    None => {
                        // Channel closed
                        break;
                    }
                }
            }
        }
    }

    session.finalize();
    info!("Session {} ended", session_id);
    Ok(())
}
