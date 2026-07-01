use crate::ServerConfig;
use crate::audio::AudioLogger;
use crate::mock_response::{
    generate_audio_response, generate_interrupted, generate_text_response, generate_tool_call,
    generate_turn_complete, generate_usage_metadata,
};
use crate::protocol::{
    ClientContent, ClientMessage, RealtimeInput, SetupComplete, SetupCompleteMessage,
    VoiceActivity, VoiceActivityMessage, VoiceActivityType,
};
use crate::vad::{Vad, VadEvent};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    AwaitingSetup,
    Ready,
    ReceivingAudio,
    Processing,
    Responding,
}

/// Events sent from the session to the server for transmission to the client
#[derive(Debug)]
pub enum SessionEvent {
    SendMessage(String),
    Close,
}

pub struct Session {
    pub id: String,
    state: SessionState,
    config: ServerConfig,
    vad: Vad,
    audio_logger: Option<AudioLogger>,
    turn_count: u32,
    audio_buffer: Vec<i16>,
    event_tx: mpsc::Sender<SessionEvent>,
    cancellation_token: CancellationToken,
    has_audio_modality: bool,
    awaiting_tool_response: bool,
}

impl Session {
    pub fn new(
        id: String,
        config: ServerConfig,
        event_tx: mpsc::Sender<SessionEvent>,
        cancellation_token: CancellationToken,
    ) -> Self {
        let audio_logger = config.audio_output_dir.as_ref().map(|dir| {
            AudioLogger::new(id.clone(), dir.clone()).expect("Failed to create audio logger")
        });

        let vad = Vad::new(config.vad_energy_threshold, config.vad_silence_frames);

        Self {
            id,
            state: SessionState::AwaitingSetup,
            config,
            vad,
            audio_logger,
            turn_count: 0,
            audio_buffer: Vec::new(),
            event_tx,
            cancellation_token,
            has_audio_modality: false,
            awaiting_tool_response: false,
        }
    }

    /// Returns a clone of the cancellation token for use in other tasks
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Cancel the session, signaling that it should end
    pub fn cancel(&self) {
        info!(session_id = %self.id, "Session cancellation requested");
        self.cancellation_token.cancel();
    }

    /// Check if the session has been cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Handle an incoming client message
    pub async fn handle_message(&mut self, msg: ClientMessage) {
        let prev_state = self.state;

        if let Some(setup) = msg.setup {
            self.handle_setup(setup).await;
        } else if let Some(client_content) = msg.client_content {
            self.handle_client_content(client_content).await;
        } else if let Some(realtime_input) = msg.realtime_input {
            self.handle_realtime_input(realtime_input).await;
        } else if let Some(tool_response) = msg.tool_response {
            self.handle_tool_response(tool_response).await;
        }

        if self.state != prev_state {
            info!(
                session_id = %self.id,
                from_state = ?prev_state,
                to_state = ?self.state,
                "State transition"
            );
        }
    }

    async fn handle_setup(&mut self, setup: crate::protocol::Setup) {
        if self.state != SessionState::AwaitingSetup {
            warn!(
                session_id = %self.id,
                current_state = ?self.state,
                "Received setup message in unexpected state"
            );
            return;
        }

        let modalities = setup
            .generation_config
            .as_ref()
            .and_then(|gc| gc.response_modalities.as_ref())
            .map(|m| m.join(","))
            .unwrap_or_else(|| "none".to_string());

        let tool_count = setup.tools.as_ref().map(|t| t.len()).unwrap_or(0);

        info!(
            session_id = %self.id,
            model = %setup.model,
            modalities = %modalities,
            tool_count = tool_count,
            "Setup received"
        );

        // Check if audio modality is requested
        if let Some(gen_config) = &setup.generation_config {
            if let Some(modalities) = &gen_config.response_modalities {
                self.has_audio_modality =
                    modalities.iter().any(|m| m.eq_ignore_ascii_case("audio"));
            }
        }

        // Send SetupComplete
        let response = SetupCompleteMessage {
            setup_complete: SetupComplete {
                session_id: self.id.clone(),
            },
        };
        self.send_json(&response).await;

        self.state = SessionState::Ready;
        info!(
            session_id = %self.id,
            has_audio = self.has_audio_modality,
            "Session ready"
        );
    }

    async fn handle_client_content(&mut self, content: ClientContent) {
        if self.state != SessionState::Ready {
            warn!(
                session_id = %self.id,
                current_state = ?self.state,
                "Received client content in unexpected state"
            );
            return;
        }

        let turn_count = content.turns.len();
        let has_text = content
            .turns
            .iter()
            .any(|t| t.parts.iter().any(|p| p.text.is_some()));

        info!(
            session_id = %self.id,
            turn_count = turn_count,
            has_text = has_text,
            turn_complete = content.turn_complete,
            "Received client content"
        );

        if content.turn_complete {
            self.state = SessionState::Processing;
            self.generate_response().await;
        }
    }

    async fn handle_realtime_input(&mut self, input: RealtimeInput) {
        // Accept audio during Ready, ReceivingAudio, or Responding states
        // Responding state allows for barge-in (interruption)
        if self.state != SessionState::Ready
            && self.state != SessionState::ReceivingAudio
            && self.state != SessionState::Responding
        {
            warn!(
                session_id = %self.id,
                current_state = ?self.state,
                "Received realtime input in unexpected state"
            );
            return;
        }

        let has_audio = input.audio.is_some();
        let audio_size = input.audio.as_ref().map(|a| a.data.len()).unwrap_or(0);

        debug!(
            session_id = %self.id,
            has_audio = has_audio,
            audio_base64_len = audio_size,
            has_activity_start = input.activity_start.is_some(),
            has_activity_end = input.activity_end.is_some(),
            audio_stream_end = input.audio_stream_end,
            "Realtime input received"
        );

        if let Some(audio) = input.audio {
            self.handle_audio_input(&audio.data).await;
        }

        if input.activity_start.is_some() {
            info!(session_id = %self.id, "Activity start signal received");
            self.state = SessionState::ReceivingAudio;
        }

        if input.activity_end.is_some() {
            info!(session_id = %self.id, "Activity end signal received");
            if self.state == SessionState::ReceivingAudio {
                self.state = SessionState::Processing;
                self.generate_response().await;
            }
        }

        if input.audio_stream_end == Some(true) {
            info!(
                session_id = %self.id,
                buffer_samples = self.audio_buffer.len(),
                "Audio stream end received"
            );
            if self.state == SessionState::ReceivingAudio && !self.audio_buffer.is_empty() {
                self.state = SessionState::Processing;
                self.generate_response().await;
            }
        }
    }

    async fn handle_audio_input(&mut self, base64_data: &str) {
        // Decode base64 audio
        let bytes = match BASE64.decode(base64_data) {
            Ok(b) => b,
            Err(e) => {
                warn!(session_id = %self.id, error = %e, "Failed to decode audio");
                return;
            }
        };

        debug!(
            session_id = %self.id,
            base64_len = base64_data.len(),
            decoded_bytes = bytes.len(),
            "Audio chunk decoded"
        );

        // Convert bytes to i16 samples (little-endian)
        let samples: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        if samples.is_empty() {
            return;
        }

        // Write to audio logger
        if let Some(ref mut logger) = self.audio_logger {
            if let Err(e) = logger.write_input(&samples) {
                warn!(session_id = %self.id, error = %e, "Failed to write input audio");
            }
        }

        // Update state if we weren't already receiving audio
        if self.state == SessionState::Ready {
            self.state = SessionState::ReceivingAudio;
        }

        // Buffer samples for response generation
        self.audio_buffer.extend_from_slice(&samples);

        // Process through VAD
        if let Some(event) = self.vad.process(&samples) {
            match event {
                VadEvent::SpeechStart => {
                    // Check if this is a barge-in (speech during response)
                    if self.state == SessionState::Responding {
                        info!(
                            session_id = %self.id,
                            discarded_samples = self.audio_buffer.len(),
                            was_awaiting_tool = self.awaiting_tool_response,
                            "VAD: Barge-in detected - interrupting response"
                        );
                        // Send interrupted message to client
                        let interrupted = generate_interrupted();
                        self.send_json(&interrupted).await;
                        // Reset tool response state if we were waiting for one
                        self.awaiting_tool_response = false;
                        self.state = SessionState::ReceivingAudio;
                    } else {
                        info!(
                            session_id = %self.id,
                            discarded_samples = self.audio_buffer.len(),
                            "VAD: Speech start detected"
                        );
                    }
                    // Clear pre-speech audio (silence/background noise) and keep only
                    // the current chunk that triggered speech detection
                    self.audio_buffer.clear();
                    self.audio_buffer.extend_from_slice(&samples);
                    self.send_voice_activity(VoiceActivityType::VoiceActivityStart)
                        .await;
                }
                VadEvent::SpeechEnd => {
                    info!(
                        session_id = %self.id,
                        buffer_samples = self.audio_buffer.len(),
                        "VAD: Speech end detected"
                    );
                    self.send_voice_activity(VoiceActivityType::VoiceActivityEnd)
                        .await;

                    // Trigger response generation
                    self.state = SessionState::Processing;
                    self.generate_response().await;
                }
            }
        }
    }

    async fn handle_tool_response(&mut self, response: crate::protocol::ToolResponse) {
        if !self.awaiting_tool_response {
            warn!(session_id = %self.id, "Received tool response but not awaiting one");
            return;
        }

        let response_count = response.function_responses.len();
        let function_names: Vec<_> = response
            .function_responses
            .iter()
            .map(|r| r.name.as_str())
            .collect();

        info!(
            session_id = %self.id,
            response_count = response_count,
            functions = ?function_names,
            "Received tool response"
        );
        self.awaiting_tool_response = false;

        // Continue with the response after tool call
        self.send_response_content().await;
    }

    async fn generate_response(&mut self) {
        self.state = SessionState::Responding;
        self.turn_count += 1;

        info!(
            session_id = %self.id,
            turn = self.turn_count,
            audio_buffer_samples = self.audio_buffer.len(),
            has_audio_modality = self.has_audio_modality,
            response_delay_ms = self.config.response_delay,
            "Generating response"
        );

        // Apply response delay, but respect cancellation
        if self.config.response_delay > 0 {
            tokio::select! {
                _ = self.cancellation_token.cancelled() => {
                    info!(session_id = %self.id, "Response generation cancelled during delay");
                    return;
                }
                _ = sleep(Duration::from_millis(self.config.response_delay)) => {}
            }
        }

        // Check if cancelled before proceeding
        if self.is_cancelled() {
            info!(session_id = %self.id, "Response generation cancelled");
            return;
        }

        // Check if we should trigger a tool call
        if let Some(interval) = self.config.tool_call_interval_option() {
            if self.turn_count % interval == 0 {
                self.send_tool_call().await;
                return;
            }
        }

        // Send normal response
        self.send_response_content().await;
    }

    async fn send_tool_call(&mut self) {
        let call_id = format!("call_{}", uuid::Uuid::new_v4());
        let tool_call = generate_tool_call(&call_id);
        self.send_json(&tool_call).await;
        self.awaiting_tool_response = true;
        info!(
            session_id = %self.id,
            call_id = %call_id,
            turn = self.turn_count,
            "Sent tool call"
        );
    }

    async fn send_response_content(&mut self) {
        // Send audio response if we have audio data and audio modality is enabled
        if self.has_audio_modality && !self.audio_buffer.is_empty() {
            match generate_audio_response(&self.audio_buffer) {
                Ok(response) => {
                    // Write output audio to logger
                    // Decode the base64 to get samples for logging
                    if let Some(model_turn) = &response.server_content.model_turn {
                        if let Some(inline_data) = &model_turn
                            .parts
                            .first()
                            .and_then(|p| p.inline_data.as_ref())
                        {
                            if let Ok(bytes) = BASE64.decode(&inline_data.data) {
                                let samples: Vec<i16> = bytes
                                    .chunks_exact(2)
                                    .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                                    .collect();
                                if let Some(ref mut logger) = self.audio_logger {
                                    if let Err(e) = logger.write_output(&samples) {
                                        warn!("Failed to write output audio: {e}");
                                    }
                                }
                            }
                        }
                    }
                    self.send_json(&response).await;
                }
                Err(e) => {
                    warn!("Failed to generate audio response: {e}");
                }
            }
        }

        // Send text response
        let text_response = generate_text_response(self.turn_count);
        self.send_json(&text_response).await;

        // Send turn complete
        let turn_complete = generate_turn_complete();
        self.send_json(&turn_complete).await;

        // Send usage metadata
        let usage = generate_usage_metadata();
        self.send_json(&usage).await;

        // Clear audio buffer and reset VAD for next turn
        self.audio_buffer.clear();
        self.vad.reset();
        self.state = SessionState::Ready;

        info!(
            session_id = %self.id,
            turn = self.turn_count,
            sent_audio = (self.has_audio_modality && !self.audio_buffer.is_empty()),
            "Turn completed"
        );
    }

    async fn send_voice_activity(&self, activity_type: VoiceActivityType) {
        let msg = VoiceActivityMessage {
            voice_activity: VoiceActivity { activity_type },
        };
        self.send_json(&msg).await;
    }

    async fn send_json<T: serde::Serialize>(&self, msg: &T) {
        match serde_json::to_string(msg) {
            Ok(json) => {
                debug!(
                    session_id = %self.id,
                    message_len = json.len(),
                    "Queueing outbound message"
                );
                if self
                    .event_tx
                    .send(SessionEvent::SendMessage(json))
                    .await
                    .is_err()
                {
                    warn!(session_id = %self.id, "Failed to send event - channel closed");
                }
            }
            Err(e) => {
                warn!(session_id = %self.id, error = %e, "Failed to serialize message");
            }
        }
    }

    /// Finalize the session and clean up resources
    pub fn finalize(self) {
        if let Some(logger) = self.audio_logger {
            if let Err(e) = logger.finalize() {
                warn!(session_id = %self.id, error = %e, "Failed to finalize audio logger");
            }
        }
        info!(
            session_id = %self.id,
            total_turns = self.turn_count,
            "Session finalized"
        );
    }
}
