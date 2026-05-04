use hound::{SampleFormat, WavSpec, WavWriter};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::PathBuf;

/// Audio logger that writes PCM audio to WAV files
pub struct AudioLogger {
    input_writer: Option<WavWriter<BufWriter<File>>>,
    output_writer: Option<WavWriter<BufWriter<File>>>,
    session_id: String,
    output_dir: PathBuf,
}

impl AudioLogger {
    pub fn new(session_id: String, output_dir: PathBuf) -> std::io::Result<Self> {
        // Create output directory if it doesn't exist
        fs::create_dir_all(&output_dir)?;

        Ok(Self {
            input_writer: None,
            output_writer: None,
            session_id,
            output_dir,
        })
    }

    /// Write input audio (16-bit PCM, 16kHz mono)
    pub fn write_input(&mut self, samples: &[i16]) -> Result<(), hound::Error> {
        let writer = match &mut self.input_writer {
            Some(w) => w,
            None => {
                let path = self
                    .output_dir
                    .join(format!("{}_input.wav", self.session_id));
                let spec = WavSpec {
                    channels: 1,
                    sample_rate: 16000,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                };
                self.input_writer = Some(WavWriter::create(path, spec)?);
                self.input_writer.as_mut().unwrap()
            }
        };

        for &sample in samples {
            writer.write_sample(sample)?;
        }

        Ok(())
    }

    /// Write output audio (16-bit PCM, 24kHz mono)
    pub fn write_output(&mut self, samples: &[i16]) -> Result<(), hound::Error> {
        let writer = match &mut self.output_writer {
            Some(w) => w,
            None => {
                let path = self
                    .output_dir
                    .join(format!("{}_output.wav", self.session_id));
                let spec = WavSpec {
                    channels: 1,
                    sample_rate: 24000,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                };
                self.output_writer = Some(WavWriter::create(path, spec)?);
                self.output_writer.as_mut().unwrap()
            }
        };

        for &sample in samples {
            writer.write_sample(sample)?;
        }

        Ok(())
    }

    /// Finalize and close the WAV files
    pub fn finalize(mut self) -> Result<(), hound::Error> {
        if let Some(writer) = self.input_writer.take() {
            writer.finalize()?;
        }
        if let Some(writer) = self.output_writer.take() {
            writer.finalize()?;
        }
        Ok(())
    }
}

impl Drop for AudioLogger {
    fn drop(&mut self) {
        // Attempt to finalize writers on drop
        if let Some(writer) = self.input_writer.take() {
            let _ = writer.finalize();
        }
        if let Some(writer) = self.output_writer.take() {
            let _ = writer.finalize();
        }
    }
}
