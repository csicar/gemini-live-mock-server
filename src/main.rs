mod audio;
mod config;
mod mock_response;
mod protocol;
mod server;
mod session;
mod vad;

use clap::Parser;
use config::Config;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::parse();

    info!("Starting Gemini Live Mock Server");
    info!("  Listen address: {}", config.listen);
    info!("  Response delay: {}ms", config.response_delay);
    info!(
        "  Tool call interval: {}",
        config
            .tool_call_interval_option()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "disabled".to_string())
    );
    info!("  VAD energy threshold: {}", config.vad_energy_threshold);
    info!("  VAD silence frames: {}", config.vad_silence_frames);
    info!("  Audio output dir: {}", config.audio_output_dir.display());

    server::run_server(config).await
}
