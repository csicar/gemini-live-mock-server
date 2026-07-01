use clap::Parser;
use gemini_live_mock_server::{ServerConfig, run_server};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug, Clone)]
#[command(name = "gemini-live-mock-server")]
#[command(about = "A mock server for the Gemini Live API")]
struct CliConfig {
    /// Address to listen on
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,

    /// Delay in milliseconds between end-of-speech detection and synthesis start
    #[arg(long, default_value = "200")]
    response_delay: u64,

    /// Trigger a tool call every N turns (0 = never)
    #[arg(long, default_value = "0")]
    tool_call_interval: u32,

    /// Energy threshold for VAD speech detection (RMS value)
    #[arg(long, default_value = "0.01")]
    vad_energy_threshold: f32,

    /// Number of silence frames before end-of-turn detection
    #[arg(long, default_value = "30")]
    vad_silence_frames: u32,

    /// Directory for audio log files (omit to disable audio logging)
    #[arg(long, default_value = "./audio_logs")]
    audio_output_dir: Option<PathBuf>,
}

impl From<CliConfig> for ServerConfig {
    fn from(cli: CliConfig) -> Self {
        ServerConfig {
            listen: cli.listen,
            response_delay: cli.response_delay,
            tool_call_interval: cli.tool_call_interval,
            vad_energy_threshold: cli.vad_energy_threshold,
            vad_silence_frames: cli.vad_silence_frames,
            audio_output_dir: cli.audio_output_dir,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli_config = CliConfig::parse();
    let config: ServerConfig = cli_config.into();

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
    info!(
        "  Audio output dir: {}",
        config
            .audio_output_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "disabled".to_string())
    );

    run_server(config).await
}
