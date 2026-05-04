use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Client Messages
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientMessage {
    #[serde(default)]
    pub setup: Option<Setup>,
    #[serde(default)]
    pub client_content: Option<ClientContent>,
    #[serde(default)]
    pub realtime_input: Option<RealtimeInput>,
    #[serde(default)]
    pub tool_response: Option<ToolResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    pub model: String,
    #[serde(default)]
    pub generation_config: Option<GenerationConfig>,
    #[serde(default)]
    pub system_instruction: Option<Content>,
    #[serde(default)]
    pub tools: Option<Vec<Tool>>,
    #[serde(default)]
    pub speech_config: Option<SpeechConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(default)]
    pub response_modalities: Option<Vec<String>>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub inline_data: Option<InlineData>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineData {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    #[serde(default)]
    pub function_declarations: Option<Vec<FunctionDeclaration>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDeclaration {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechConfig {
    #[serde(default)]
    pub voice_config: Option<VoiceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfig {
    #[serde(default)]
    pub prebuilt_voice_config: Option<PrebuiltVoiceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrebuiltVoiceConfig {
    pub voice_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientContent {
    #[serde(default)]
    pub turns: Vec<Content>,
    #[serde(default)]
    pub turn_complete: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeInput {
    #[serde(default)]
    pub audio: Option<Blob>,
    #[serde(default)]
    pub video: Option<Blob>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub activity_start: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub activity_end: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub audio_stream_end: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blob {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResponse {
    pub function_responses: Vec<FunctionResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionResponse {
    pub id: String,
    pub name: String,
    pub response: serde_json::Value,
}

// ============================================================================
// Server Messages
// ============================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupCompleteMessage {
    pub setup_complete: SetupComplete,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupComplete {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerContentMessage {
    pub server_content: ServerContent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_turn: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupted: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallMessage {
    pub tool_call: ToolCall,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub function_calls: Vec<FunctionCall>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadataMessage {
    pub usage_metadata: UsageMetadata,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    pub prompt_token_count: u32,
    pub response_token_count: u32,
    pub total_token_count: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceActivityMessage {
    pub voice_activity: VoiceActivity,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceActivity {
    #[serde(rename = "type")]
    pub activity_type: VoiceActivityType,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VoiceActivityType {
    VoiceActivityStart,
    VoiceActivityEnd,
}
