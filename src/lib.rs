//! # Gemini Live Mock Server
//!
//! A mock server for the [Gemini Live API](https://ai.google.dev/api/multimodal-live),
//! useful for testing applications that integrate with Gemini's real-time bidirectional
//! WebSocket communication.
//!
//! ## Features
//!
//! - WebSocket server that mimics the Gemini Live API protocol
//! - Voice Activity Detection (VAD) for automatic turn-taking
//! - Audio resampling (16kHz input → 24kHz output)
//! - Configurable response delays and tool call simulation
//! - Audio logging for debugging
//! - Barge-in (interruption) support
//!
//! ## Quick Start
//!
//! Add to your `Cargo.toml`:
//!
//! ```toml
//! [dev-dependencies]
//! gemini-live-mock-server = { version = "0.1", default-features = false }
//! tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
//! ```
//!
//! ## Basic Usage
//!
//! ```rust,no_run
//! use gemini_live_mock_server::{ServerConfig, run_server};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = ServerConfig::default();
//!     run_server(config).await
//! }
//! ```
//!
//! ## Integration Testing
//!
//! The primary use case is running the mock server during integration tests:
//!
//! ```rust,no_run
//! use gemini_live_mock_server::{ServerConfig, run_server};
//! use std::net::SocketAddr;
//!
//! #[tokio::test]
//! async fn test_my_gemini_client() {
//!     // Start mock server on a random available port
//!     let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
//!     let config = ServerConfig {
//!         listen: addr,
//!         response_delay: 50,  // Fast responses for tests
//!         ..Default::default()
//!     };
//!
//!     // Spawn the server in a background task
//!     let server_handle = tokio::spawn(async move {
//!         run_server(config).await.unwrap();
//!     });
//!
//!     // Give server time to start
//!     tokio::time::sleep(std::time::Duration::from_millis(100)).await;
//!
//!     // Run your client tests against ws://127.0.0.1:<port>/ws
//!     // ...
//!
//!     server_handle.abort();
//! }
//! ```
//!
//! ## Configuration
//!
//! [`ServerConfig`] provides the following options:
//!
//! | Field | Default | Description |
//! |-------|---------|-------------|
//! | `listen` | `127.0.0.1:8080` | Socket address to bind the server |
//! | `response_delay` | `200` | Milliseconds to wait before generating a response |
//! | `tool_call_interval` | `None` | Emit a tool call every N turns (`None` = disabled) |
//! | `vad_energy_threshold` | `0.01` | RMS energy threshold for speech detection |
//! | `vad_silence_frames` | `30` | Silence frames before end-of-turn |
//! | `audio_output_dir` | `None` | Directory to write audio log files (disabled if `None`) |
//!
//! ## Protocol
//!
//! The server implements the Gemini Live WebSocket protocol:
//!
//! ### Client → Server Messages
//!
//! - **Setup**: Initialize session with model configuration
//! - **ClientContent**: Send text content with turn completion signal
//! - **RealtimeInput**: Stream audio data with activity signals
//! - **ToolResponse**: Respond to tool calls from the server
//!
//! ### Server → Client Messages
//!
//! - **SetupComplete**: Confirms session is ready
//! - **ServerContent**: Text or audio response content
//! - **ToolCall**: Request for tool execution
//! - **VoiceActivity**: Speech start/end notifications
//! - **UsageMetadata**: Token usage information
//!
//! ## Simulating Tool Calls
//!
//! To test tool call handling, set `tool_call_interval`:
//!
//! ```rust
//! use gemini_live_mock_server::ServerConfig;
//!
//! let config = ServerConfig {
//!     tool_call_interval: Some(2),  // Every 2nd turn triggers a tool call
//!     ..Default::default()
//! };
//! ```
//!
//! The mock server will emit a `get_current_weather` tool call that your client
//! should handle and respond to with a `ToolResponse` message.
//!
//! ## Simulating a Server-Initiated Disconnect
//!
//! To test how a client handles Gemini closing the connection (e.g. with a specific
//! close code/reason, or an abrupt drop), start the server with
//! [`run_server_with_control`] instead of [`run_server`]:
//!
//! ```rust,no_run
//! use gemini_live_mock_server::{CloseMode, ServerConfig, run_server_with_control};
//!
//! # async fn example() {
//! let config = ServerConfig::default();
//! let (server, control) = run_server_with_control(
//!     config,
//!     CloseMode::Frame { code: 1008, reason: "policy violation".to_string() },
//! );
//! tokio::spawn(async move { server.await.unwrap(); });
//!
//! // ... connect a client and drive the interaction ...
//!
//! control.trigger_close();
//! # }
//! ```
//!
//! [`CloseMode`] covers three distinct wire-level scenarios:
//!
//! - `Frame { code, reason }` - a close handshake carrying an explicit code and reason.
//! - `EmptyFrame` - a close handshake with no payload at all (many clients surface this as
//!   close code 1005, "no status received").
//! - `Abrupt` - the connection is dropped without any close handshake, simulating a crash
//!   or network failure (many clients surface this as close code 1006, "abnormal closure").
//!
//! ## Observing Received Messages
//!
//! The same [`ServerControl`] handle returned by [`run_server_with_control`] can also be
//! used to inspect the raw text of every client message the active session has received
//! so far, via [`ServerControl::received_messages`]. This is useful for asserting that a
//! particular message actually reached the server - e.g. one sent by a client during a
//! shutdown grace period:
//!
//! ```rust,no_run
//! # use gemini_live_mock_server::{CloseMode, ServerConfig, run_server_with_control};
//! # async fn example() {
//! # let config = ServerConfig::default();
//! # let (server, control) = run_server_with_control(
//! #     config,
//! #     CloseMode::Frame { code: 1008, reason: "policy violation".to_string() },
//! # );
//! # tokio::spawn(async move { server.await.unwrap(); });
//! // ... connect a client and send some messages ...
//!
//! let messages = control.received_messages();
//! assert!(messages.iter().any(|m| m.contains("expected marker")));
//! # }
//! ```
//!
//! ## WebSocket Endpoints
//!
//! - `ws://<addr>/ws` - Main WebSocket endpoint
//! - `ws://<addr>/` - Alternative WebSocket endpoint
//! - `http://<addr>/health` - Health check endpoint (returns "OK")

mod audio;
mod mock_response;
mod protocol;
mod server;
mod session;
mod vad;

use std::net::SocketAddr;
use std::path::PathBuf;

pub use protocol::{
    ClientContent, ClientMessage, RealtimeInput, ServerContentMessage, SetupComplete,
    SetupCompleteMessage, ToolCall, ToolCallMessage, ToolResponse, VoiceActivity,
    VoiceActivityMessage, VoiceActivityType,
};
pub use server::{CloseMode, ServerControl, run_server, run_server_with_control};
pub use session::{Session, SessionEvent, SessionState};

/// Configuration for the mock server.
///
/// This struct contains all settings needed to run the mock server.
/// Use [`Default::default()`] for sensible defaults, or customize individual fields.
///
/// # Example
///
/// ```rust
/// use gemini_live_mock_server::ServerConfig;
/// use std::path::PathBuf;
///
/// let config = ServerConfig {
///     listen: "0.0.0.0:9000".parse().unwrap(),
///     response_delay: 100,
///     tool_call_interval: Some(3),  // Tool call every 3rd turn
///     audio_output_dir: Some(PathBuf::from("/tmp/test_audio")),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Socket address to bind the WebSocket server.
    ///
    /// Use `127.0.0.1:0` to let the OS assign an available port.
    pub listen: SocketAddr,

    /// Delay in milliseconds between end-of-speech detection and response generation.
    ///
    /// Simulates the processing time of the real Gemini API. Set to a low value
    /// (e.g., 50ms) for faster tests.
    pub response_delay: u64,

    /// Emit a tool call every N turns. Set to `None` to disable tool calls.
    ///
    /// When enabled, the server sends a `get_current_weather` tool call instead
    /// of a normal response on every Nth turn. The client must respond with a
    /// `ToolResponse` before the server continues.
    pub tool_call_interval: Option<u32>,

    /// RMS energy threshold for Voice Activity Detection (VAD).
    ///
    /// Audio frames with energy above this threshold are considered speech.
    /// Lower values make detection more sensitive. Range: 0.0 to 1.0.
    pub vad_energy_threshold: f32,

    /// Number of consecutive silence frames required to trigger end-of-turn.
    ///
    /// Higher values prevent premature turn-ending during brief pauses.
    /// At 16kHz with typical frame sizes, 30 frames ≈ 0.5 seconds.
    pub vad_silence_frames: u32,

    /// Directory where audio log files are written.
    ///
    /// Each session creates two WAV files: `{session_id}_input.wav` (16kHz)
    /// and `{session_id}_output.wav` (24kHz). Useful for debugging audio issues.
    /// Set to `None` to disable audio logging entirely.
    pub audio_output_dir: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".parse().unwrap(),
            response_delay: 200,
            tool_call_interval: None,
            vad_energy_threshold: 0.01,
            vad_silence_frames: 30,
            audio_output_dir: None,
        }
    }
}
