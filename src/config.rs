use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "gemini-live-mock-server")]
#[command(about = "A mock server for the Gemini Live API")]
pub struct Config {
    /// Address to listen on
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,

    /// Delay in milliseconds between end-of-speech detection and synthesis start
    #[arg(long, default_value = "200")]
    pub response_delay: u64,

    /// Trigger a tool call every N turns (0 = never)
    #[arg(long, default_value = "0")]
    pub tool_call_interval: u32,

    /// Energy threshold for VAD speech detection (RMS value)
    #[arg(long, default_value = "0.01")]
    pub vad_energy_threshold: f32,

    /// Number of silence frames before end-of-turn detection
    #[arg(long, default_value = "30")]
    pub vad_silence_frames: u32,

    /// Directory for audio log files
    #[arg(long, default_value = "./audio_logs")]
    pub audio_output_dir: PathBuf,
}

impl Config {
    pub fn tool_call_interval_option(&self) -> Option<u32> {
        if self.tool_call_interval == 0 {
            None
        } else {
            Some(self.tool_call_interval)
        }
    }
}
