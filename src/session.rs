use crate::audio::AudioLogger;
use crate::config::Config;
use crate::mock_response::{
    generate_audio_response, generate_text_response, generate_tool_call, generate_turn_complete,
    generate_usage_metadata,
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
    config: Config,
    vad: Vad,
    audio_logger: AudioLogger,
    turn_count: u32,
    audio_buffer: Vec<i16>,
    event_tx: mpsc::Sender<SessionEvent>,
    has_audio_modality: bool,
    awaiting_tool_response: bool,
}

impl Session {
    pub fn new(id: String, config: Config, event_tx: mpsc::Sender<SessionEvent>) -> Self {
        let audio_logger =
            AudioLogger::new(id.clone(), config.audio_output_dir.clone()).expect("Failed to create audio logger");

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
            has_audio_modality: false,
            awaiting_tool_response: false,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Handle an incoming client message
    pub async fn handle_message(&mut self, msg: ClientMessage) {
        debug!("Session {} handling message in state {:?}", self.id, self.state);

        if let Some(setup) = msg.setup {
            self.handle_setup(setup).await;
        } else if let Some(client_content) = msg.client_content {
            self.handle_client_content(client_content).await;
        } else if let Some(realtime_input) = msg.realtime_input {
            self.handle_realtime_input(realtime_input).await;
        } else if let Some(tool_response) = msg.tool_response {
            self.handle_tool_response(tool_response).await;
        }
    }

    async fn handle_setup(&mut self, setup: crate::protocol::Setup) {
        if self.state != SessionState::AwaitingSetup {
            warn!("Received setup message in state {:?}", self.state);
            return;
        }

        info!("Session {} setup with model: {}", self.id, setup.model);

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
        info!("Session {} ready", self.id);
    }

    async fn handle_client_content(&mut self, content: ClientContent) {
        if self.state != SessionState::Ready {
            warn!("Received client content in state {:?}", self.state);
            return;
        }

        info!("Session {} received client content, turn_complete: {}", self.id, content.turn_complete);

        if content.turn_complete {
            self.state = SessionState::Processing;
            self.generate_response().await;
        }
    }

    async fn handle_realtime_input(&mut self, input: RealtimeInput) {
        if self.state != SessionState::Ready && self.state != SessionState::ReceivingAudio {
            warn!("Received realtime input in state {:?}", self.state);
            return;
        }

        if let Some(audio) = input.audio {
            self.handle_audio_input(&audio.data).await;
        }

        if input.activity_start.is_some() {
            debug!("Activity start signal received");
            self.state = SessionState::ReceivingAudio;
        }

        if input.activity_end.is_some() {
            debug!("Activity end signal received");
            if self.state == SessionState::ReceivingAudio {
                self.state = SessionState::Processing;
                self.generate_response().await;
            }
        }

        if input.audio_stream_end == Some(true) {
            debug!("Audio stream end received");
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
                warn!("Failed to decode audio: {e}");
                return;
            }
        };

        // Convert bytes to i16 samples (little-endian)
        let samples: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        if samples.is_empty() {
            return;
        }

        // Write to audio logger
        if let Err(e) = self.audio_logger.write_input(&samples) {
            warn!("Failed to write input audio: {e}");
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
                    info!("VAD: Speech start detected");
                    self.send_voice_activity(VoiceActivityType::VoiceActivityStart).await;
                }
                VadEvent::SpeechEnd => {
                    info!("VAD: Speech end detected");
                    self.send_voice_activity(VoiceActivityType::VoiceActivityEnd).await;

                    // Trigger response generation
                    self.state = SessionState::Processing;
                    self.generate_response().await;
                }
            }
        }
    }

    async fn handle_tool_response(&mut self, _response: crate::protocol::ToolResponse) {
        if !self.awaiting_tool_response {
            warn!("Received tool response but not awaiting one");
            return;
        }

        info!("Session {} received tool response", self.id);
        self.awaiting_tool_response = false;

        // Continue with the response after tool call
        self.send_response_content().await;
    }

    async fn generate_response(&mut self) {
        self.state = SessionState::Responding;
        self.turn_count += 1;

        info!("Session {} generating response for turn {}", self.id, self.turn_count);

        // Apply response delay
        if self.config.response_delay > 0 {
            sleep(Duration::from_millis(self.config.response_delay)).await;
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
        info!("Session {} sent tool call {}", self.id, call_id);
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
                                if let Err(e) = self.audio_logger.write_output(&samples) {
                                    warn!("Failed to write output audio: {e}");
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

        info!("Session {} completed turn {}", self.id, self.turn_count);
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
                if self.event_tx.send(SessionEvent::SendMessage(json)).await.is_err() {
                    warn!("Failed to send event - channel closed");
                }
            }
            Err(e) => {
                warn!("Failed to serialize message: {e}");
            }
        }
    }

    /// Finalize the session and clean up resources
    pub fn finalize(self) {
        if let Err(e) = self.audio_logger.finalize() {
            warn!("Failed to finalize audio logger: {e}");
        }
        info!("Session {} finalized", self.id);
    }
}
