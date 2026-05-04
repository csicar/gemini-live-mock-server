use crate::protocol::{
    Content, FunctionCall, InlineData, Part, ServerContent, ServerContentMessage, ToolCall,
    ToolCallMessage, UsageMetadata, UsageMetadataMessage,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use rubato::{FftFixedIn, Resampler};

/// Generate a mock text response
pub fn generate_text_response(turn_number: u32) -> ServerContentMessage {
    ServerContentMessage {
        server_content: ServerContent {
            model_turn: Some(Content {
                role: Some("model".to_string()),
                parts: vec![Part {
                    text: Some(format!("Mock response #{turn_number}")),
                    inline_data: None,
                }],
            }),
            turn_complete: None,
            interrupted: None,
        },
    }
}

/// Generate a turn complete message
pub fn generate_turn_complete() -> ServerContentMessage {
    ServerContentMessage {
        server_content: ServerContent {
            model_turn: None,
            turn_complete: Some(true),
            interrupted: None,
        },
    }
}

/// Generate mock usage metadata
pub fn generate_usage_metadata() -> UsageMetadataMessage {
    UsageMetadataMessage {
        usage_metadata: UsageMetadata {
            prompt_token_count: 10,
            response_token_count: 5,
            total_token_count: 15,
        },
    }
}

/// Generate a mock tool call
pub fn generate_tool_call(call_id: &str) -> ToolCallMessage {
    ToolCallMessage {
        tool_call: ToolCall {
            function_calls: vec![FunctionCall {
                id: call_id.to_string(),
                name: "get_data".to_string(),
                args: serde_json::json!({
                    "query": "mock_query"
                }),
            }],
        },
    }
}

/// Generate an audio response by upsampling input audio from 16kHz to 24kHz
pub fn generate_audio_response(input_samples: &[i16]) -> Result<ServerContentMessage, String> {
    let output_samples = resample_16k_to_24k(input_samples)?;

    // Encode as base64
    let bytes: Vec<u8> = output_samples
        .iter()
        .flat_map(|&s| s.to_le_bytes())
        .collect();
    let encoded = BASE64.encode(&bytes);

    Ok(ServerContentMessage {
        server_content: ServerContent {
            model_turn: Some(Content {
                role: Some("model".to_string()),
                parts: vec![Part {
                    text: None,
                    inline_data: Some(InlineData {
                        mime_type: "audio/pcm;rate=24000".to_string(),
                        data: encoded,
                    }),
                }],
            }),
            turn_complete: None,
            interrupted: None,
        },
    })
}

/// Resample audio from 16kHz to 24kHz
fn resample_16k_to_24k(input: &[i16]) -> Result<Vec<i16>, String> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    // Convert to f64 for rubato
    let input_f64: Vec<f64> = input.iter().map(|&s| s as f64 / i16::MAX as f64).collect();

    // Calculate chunk size based on rubato's requirements
    // Using a reasonable chunk size that works well with FFT
    let chunk_size = 1024;

    let mut resampler = FftFixedIn::<f64>::new(16000, 24000, chunk_size, 2, 1)
        .map_err(|e| format!("Failed to create resampler: {e}"))?;

    let mut output_f64 = Vec::new();

    // Process in chunks
    let mut pos = 0;
    while pos < input_f64.len() {
        let end = (pos + chunk_size).min(input_f64.len());
        let mut chunk = input_f64[pos..end].to_vec();

        // Pad if necessary
        if chunk.len() < chunk_size {
            chunk.resize(chunk_size, 0.0);
        }

        let waves_in = vec![chunk];
        match resampler.process(&waves_in, None) {
            Ok(waves_out) => {
                if !waves_out.is_empty() {
                    output_f64.extend_from_slice(&waves_out[0]);
                }
            }
            Err(e) => {
                tracing::warn!("Resampling error: {e}");
            }
        }

        pos += chunk_size;
    }

    // Convert back to i16
    let output: Vec<i16> = output_f64
        .iter()
        .map(|&s| (s * i16::MAX as f64).clamp(i16::MIN as f64, i16::MAX as f64) as i16)
        .collect();

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_text_response() {
        let response = generate_text_response(1);
        let content = response.server_content.model_turn.unwrap();
        assert_eq!(content.role, Some("model".to_string()));
        assert_eq!(content.parts[0].text, Some("Mock response #1".to_string()));
    }

    #[test]
    fn test_generate_tool_call() {
        let tool_call = generate_tool_call("test_id");
        assert_eq!(tool_call.tool_call.function_calls.len(), 1);
        assert_eq!(tool_call.tool_call.function_calls[0].id, "test_id");
        assert_eq!(tool_call.tool_call.function_calls[0].name, "get_data");
    }

    #[test]
    fn test_resample_empty() {
        let result = resample_16k_to_24k(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
