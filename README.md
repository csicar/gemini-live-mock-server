# Gemini Live API Mock Server

A WebSocket mock server that implements the Gemini Live API protocol for testing speech-to-speech bots without consuming API tokens.

## Features

- **Full Gemini Live API protocol support** - Setup, client content, realtime input, tool calls
- **Configurable response delays** - Simulate network/model latency
- **Tool call support** - Trigger tool calls at configurable intervals
- **Energy-based VAD** - Detect end-of-turn using audio energy levels
- **Audio logging** - Write input/output audio to WAV files for analysis
- **Echo mode** - Returns input audio upsampled from 16kHz to 24kHz
- **Session close control** - From Rust test code, trigger the active session's connection to close with a specific WebSocket close code/reason, or drop abruptly (see [Simulating a Server-Initiated Disconnect](#simulating-a-server-initiated-disconnect))

## Installation

```bash
cargo build --release
```

## Usage

```bash
# Basic usage with defaults
cargo run

# With custom configuration
cargo run -- \
    --listen 127.0.0.1:8080 \
    --response-delay 500 \
    --tool-call-interval 3 \
    --vad-energy-threshold 0.01 \
    --vad-silence-frames 30 \
    --audio-output-dir ./audio_logs
```

### Command Line Options

| Option | Default | Description |
|--------|---------|-------------|
| `--listen` | `127.0.0.1:8080` | Address to listen on |
| `--response-delay` | `200` | Delay (ms) between end-of-speech and response |
| `--tool-call-interval` | `0` | Trigger tool call every N turns (0 = disabled) |
| `--vad-energy-threshold` | `0.01` | RMS energy threshold for speech detection |
| `--vad-silence-frames` | `30` | Frames of silence before end-of-turn |
| `--audio-output-dir` | `./audio_logs` | Directory for WAV audio logs |

## Testing

### With websocat

```bash
# Install websocat
cargo install websocat

# Connect to the mock server
websocat ws://127.0.0.1:8080
```

Send a setup message:
```json
{"setup":{"model":"gemini-live-2.5-flash-preview","generationConfig":{"responseModalities":["TEXT"]}}}
```

Send content:
```json
{"clientContent":{"turns":[{"role":"user","parts":[{"text":"Hello!"}]}],"turnComplete":true}}
```

### With Python

```python
import asyncio
import websockets
import json

async def test_mock_server():
    async with websockets.connect("ws://127.0.0.1:8080") as ws:
        # Setup
        await ws.send(json.dumps({
            "setup": {
                "model": "gemini-live-2.5-flash-preview",
                "generationConfig": {"responseModalities": ["TEXT"]}
            }
        }))
        print("Setup response:", await ws.recv())

        # Send content
        await ws.send(json.dumps({
            "clientContent": {
                "turns": [{"role": "user", "parts": [{"text": "Hello!"}]}],
                "turnComplete": True
            }
        }))

        # Receive responses
        while True:
            msg = await ws.recv()
            data = json.loads(msg)
            print("Received:", data)
            if data.get("serverContent", {}).get("turnComplete"):
                break

asyncio.run(test_mock_server())
```

### Simulating a Server-Initiated Disconnect

For Rust integration tests that need to verify how a client handles Gemini closing the
connection (e.g. with a specific close code/reason, or an abrupt drop), start the server
with `run_server_with_control` instead of `run_server`:

```rust
use gemini_live_mock_server::{CloseMode, ServerConfig, run_server_with_control};

let config = ServerConfig::default();
let (server, control) = run_server_with_control(
    config,
    CloseMode::Frame { code: 1008, reason: "policy violation".to_string() },
);
tokio::spawn(async move { server.await.unwrap(); });

// ... connect a client and drive the interaction ...

control.trigger_close();
```

Use `CloseMode::Abrupt` instead to drop the connection without a close handshake,
simulating a crash or network failure rather than a clean shutdown.

`ServerControl` reaches into whichever session is currently active, so this is intended for
test scenarios with one connection at a time.

## Audio Formats

- **Input**: PCM 16-bit mono, 16kHz (matches Gemini Live API)
- **Output**: PCM 16-bit mono, 24kHz (matches Gemini Live API)

## Audio Logging

When audio is received, the server writes WAV files to the configured output directory:

- `{session_id}_input.wav` - Input audio at 16kHz
- `{session_id}_output.wav` - Output audio at 24kHz

## Protocol Support

### Supported Client Messages

- `setup` - Session configuration
- `clientContent` - Turn-based text/image content
- `realtimeInput` - Streaming audio/video input
- `toolResponse` - Function call responses

### Supported Server Messages

- `setupComplete` - Session established
- `serverContent` - Model responses (text/audio)
- `toolCall` - Function call requests
- `usageMetadata` - Token usage statistics
- `voiceActivity` - VAD events (start/end)

## License

MIT
