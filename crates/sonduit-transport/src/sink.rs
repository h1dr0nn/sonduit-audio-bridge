//! Audio sinks for the receiver side.
//!
//! The real sink is AAudio on Android. Neither this machine nor CI has an
//! Android device, so the walking skeleton ends in [`WavFileSink`] and the
//! test asserts on the file's contents. See `docs/environment.md`: this is a
//! deliberate substitute for an audibility check that cannot be performed
//! here, not a claim that the audio path works end to end.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use sonduit_core::format::Format;

/// Somewhere decoded PCM can be written.
pub trait AudioSink {
    /// Write one block of interleaved PCM.
    ///
    /// # Errors
    /// Implementation defined; a file sink reports I/O failures.
    fn write(&mut self, pcm: &[u8]) -> io::Result<()>;

    /// Flush and finish. Must be called; a WAV file is invalid without it.
    ///
    /// # Errors
    /// Implementation defined.
    fn finish(&mut self) -> io::Result<()>;
}

/// Discards everything. Useful for throughput measurement.
#[derive(Debug, Default)]
pub struct NullSink {
    bytes: u64,
}

impl NullSink {
    /// Bytes discarded so far.
    #[must_use]
    pub const fn bytes_written(&self) -> u64 {
        self.bytes
    }
}

impl AudioSink for NullSink {
    fn write(&mut self, pcm: &[u8]) -> io::Result<()> {
        self.bytes += pcm.len() as u64;
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes a canonical 44-byte-header RIFF/WAVE file.
#[derive(Debug)]
pub struct WavFileSink {
    writer: BufWriter<File>,
    format: Format,
    data_bytes: u32,
    finished: bool,
}

/// Size of the canonical WAV header this sink writes.
pub const WAV_HEADER_BYTES: usize = 44;

impl WavFileSink {
    /// Create a WAV file at `path`.
    ///
    /// The header is written immediately with placeholder sizes and rewritten
    /// by [`AudioSink::finish`], which is why the sink needs a seekable file
    /// rather than an arbitrary writer.
    ///
    /// # Errors
    /// Propagates file creation and write failures.
    pub fn create(path: impl AsRef<Path>, format: Format) -> io::Result<Self> {
        let file = File::create(path)?;
        let mut sink = Self {
            writer: BufWriter::new(file),
            format,
            data_bytes: 0,
            finished: false,
        };
        sink.write_header()?;
        Ok(sink)
    }

    fn write_header(&mut self) -> io::Result<()> {
        let channels = u16::from(self.format.channels);
        let bits = u16::from(self.format.bit_depth.bits());
        let block_align = channels * (bits / 8);
        let byte_rate = self.format.sample_rate * u32::from(block_align);

        self.writer.write_all(b"RIFF")?;
        self.writer
            .write_all(&(36 + self.data_bytes).to_le_bytes())?;
        self.writer.write_all(b"WAVE")?;

        self.writer.write_all(b"fmt ")?;
        self.writer.write_all(&16_u32.to_le_bytes())?;
        self.writer.write_all(&1_u16.to_le_bytes())?; // PCM
        self.writer.write_all(&channels.to_le_bytes())?;
        self.writer
            .write_all(&self.format.sample_rate.to_le_bytes())?;
        self.writer.write_all(&byte_rate.to_le_bytes())?;
        self.writer.write_all(&block_align.to_le_bytes())?;
        self.writer.write_all(&bits.to_le_bytes())?;

        self.writer.write_all(b"data")?;
        self.writer.write_all(&self.data_bytes.to_le_bytes())?;
        Ok(())
    }

    /// PCM bytes written so far, excluding the header.
    #[must_use]
    pub const fn data_bytes(&self) -> u32 {
        self.data_bytes
    }
}

impl AudioSink for WavFileSink {
    fn write(&mut self, pcm: &[u8]) -> io::Result<()> {
        self.writer.write_all(pcm)?;
        self.data_bytes += pcm.len() as u32;
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.writer.flush()?;
        self.writer.seek(SeekFrom::Start(0))?;
        self.write_header()?;
        self.writer.flush()?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for WavFileSink {
    fn drop(&mut self) {
        // A WAV whose header still says zero bytes is unreadable, so a caller
        // that forgot to finish gets a valid file anyway.
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("sonduit-{name}-{}.wav", std::process::id()));
        path
    }

    fn read_u32(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    fn read_u16(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    #[test]
    fn the_header_describes_the_format_and_the_real_data_size() {
        let path = temp_path("header");
        let format = Format::stereo_48k();
        let payload = vec![0xAB_u8; 400];

        {
            let mut sink = WavFileSink::create(&path, format).unwrap();
            sink.write(&payload).unwrap();
            sink.finish().unwrap();
        }

        let mut bytes = Vec::new();
        File::open(&path).unwrap().read_to_end(&mut bytes).unwrap();

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(&bytes[36..40], b"data");

        assert_eq!(read_u32(&bytes, 4), 36 + 400, "RIFF size");
        assert_eq!(read_u16(&bytes, 20), 1, "PCM tag");
        assert_eq!(read_u16(&bytes, 22), 2, "channels");
        assert_eq!(read_u32(&bytes, 24), 48_000, "sample rate");
        assert_eq!(read_u32(&bytes, 28), 48_000 * 4, "byte rate");
        assert_eq!(read_u16(&bytes, 32), 4, "block align");
        assert_eq!(read_u16(&bytes, 34), 16, "bits");
        assert_eq!(read_u32(&bytes, 40), 400, "data size");

        assert_eq!(bytes.len(), WAV_HEADER_BYTES + 400);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dropping_without_finishing_still_produces_a_valid_size() {
        let path = temp_path("drop");
        {
            let mut sink = WavFileSink::create(&path, Format::stereo_48k()).unwrap();
            sink.write(&[0_u8; 64]).unwrap();
            // No finish() call; Drop must repair the header.
        }

        let mut bytes = Vec::new();
        File::open(&path).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(read_u32(&bytes, 40), 64);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn null_sink_counts_without_storing() {
        let mut sink = NullSink::default();
        sink.write(&[0; 10]).unwrap();
        sink.write(&[0; 22]).unwrap();
        sink.finish().unwrap();
        assert_eq!(sink.bytes_written(), 32);
    }
}
