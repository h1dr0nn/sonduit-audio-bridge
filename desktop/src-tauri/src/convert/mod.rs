//! Audio processing for the editor screens.
//!
//! # Why FFmpeg, and why as a subprocess
//!
//! The editor offers six containers, EBU R128 loudness normalisation,
//! compression, limiting, silence removal, time stretching and pitch shifting.
//! Reimplementing that in Rust is a project of its own; FFmpeg does all of it
//! and is the same engine the previous Python backend drove.
//!
//! It runs as a **separate process**, never linked. That keeps the licence
//! boundary at a process boundary, which is what lets an MIT application use an
//! LGPL or GPL build of FFmpeg. See `docs/licensing.md`.
//!
//! The argument construction lives in [`args`] and is pure, so the whole
//! mapping from a UI job to a command line is unit tested without FFmpeg
//! present.

mod args;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    /// Change container and codec.
    Convert,
    /// Loudness normalise, compress, limit.
    Master,
    /// Remove leading and trailing silence.
    Trim,
    /// Change speed, pitch, or cut a range.
    Modify,
    /// Measure the input and suggest a preset.
    Analyze,
}

/// Mastering knobs, all optional so a partial payload still decodes.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct MasterParameters {
    /// Integrated loudness target, in LUFS.
    pub target_lufs: Option<f64>,
    /// Whether to run a compressor.
    pub apply_compression: Option<bool>,
    /// Whether to run a limiter.
    pub apply_limiter: Option<bool>,
    /// Gain applied after everything else, in dB.
    pub output_gain: Option<f64>,
}

/// One job, as the editor screen sends it.
///
/// Field names match what the frontend already posts; unknown fields are
/// ignored so the two sides can move independently.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConvertPayload {
    /// What to do.
    pub operation: Operation,
    /// Absolute paths of the inputs.
    pub files: Vec<String>,
    /// Directory the results go into.
    pub output: String,
    /// Target format, for [`Operation::Convert`].
    #[serde(default)]
    pub format: Option<String>,
    /// Mastering knobs.
    #[serde(default)]
    pub parameters: MasterParameters,
    /// Silence threshold in dBFS.
    #[serde(default)]
    pub silence_threshold: Option<f64>,
    /// Shortest silence worth removing, in milliseconds.
    #[serde(default)]
    pub minimum_silence_ms: Option<u32>,
    /// Silence to leave in place, in milliseconds.
    #[serde(default)]
    pub padding_ms: Option<u32>,
    /// Playback rate multiplier.
    #[serde(default)]
    pub speed: Option<f64>,
    /// Pitch shift in semitones.
    #[serde(default)]
    pub pitch: Option<f64>,
    /// Cut start, in seconds.
    #[serde(default)]
    pub cut_start: Option<f64>,
    /// Cut end, in seconds.
    #[serde(default)]
    pub cut_end: Option<f64>,
    /// Whether the cut range applies at all.
    #[serde(default)]
    pub is_cut_enabled: Option<bool>,
}

/// What a command returns to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct BackendResult {
    /// `"success"` or `"error"`.
    pub status: String,
    /// Human-readable summary.
    pub message: String,
    /// Per-file payload, used by analysis.
    pub data: Vec<AnalysisEntry>,
}

/// One analysed file.
#[derive(Debug, Clone, Serialize)]
pub struct AnalysisEntry {
    /// The file that was measured.
    pub file: String,
    /// Measured integrated loudness, in LUFS.
    pub lufs: f64,
    /// Preset name the measurement suggests.
    pub suggestion: String,
}

/// Progress and completion notifications, emitted as `conversion-progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum ProgressEvent {
    Progress {
        event: &'static str,
        operation_type: String,
        index: usize,
        total: usize,
        file: String,
        status: &'static str,
    },
    Complete {
        event: &'static str,
        operation_type: String,
        status: &'static str,
        message: String,
        outputs: Vec<String>,
    },
}

/// Anything that can go wrong running a job.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    /// FFmpeg could not be found anywhere.
    ///
    /// The wording matters: the editor screen looks for "install ffmpeg" to
    /// decide whether to show its tools-missing message.
    #[error(
        "FFmpeg could not be located. Install FFmpeg and make sure it is on PATH, \
         or place it next to the application."
    )]
    FfmpegMissing,

    /// The requested output format has no encoder mapping.
    #[error("unsupported output format: {0}")]
    UnsupportedFormat(String),

    /// A path had no usable file name.
    #[error("cannot derive an output name from {0}")]
    BadPath(String),

    /// FFmpeg ran and failed.
    #[error("{file}: {detail}")]
    Failed {
        /// Input that failed.
        file: String,
        /// Last meaningful line of FFmpeg's output.
        detail: String,
    },

    /// The job named no inputs.
    #[error("no input files")]
    NoInputs,

    /// Spawning the process failed.
    #[error("could not run FFmpeg: {0}")]
    Spawn(String),
}

/// Locate an FFmpeg executable.
///
/// A copy shipped beside the application wins, so a bundled build never
/// depends on what happens to be installed. Otherwise fall back to `PATH`.
fn ffmpeg_path(app: &AppHandle) -> Result<PathBuf, ConvertError> {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };

    if let Ok(resources) = app.path().resource_dir() {
        let bundled = resources.join("binaries").join(name);
        if bundled.is_file() {
            return Ok(bundled);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join(name);
            if beside.is_file() {
                return Ok(beside);
            }
        }
    }

    // `-version` is the cheapest way to ask whether PATH resolves it.
    let found = std::process::Command::new(name)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok();

    if found {
        Ok(PathBuf::from(name))
    } else {
        Err(ConvertError::FfmpegMissing)
    }
}

/// Keep the last line that looks like a reason, for the error message.
fn last_meaningful_line(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        // Progress lines carry no reason, so they are never the explanation.
        .rfind(|line| !line.is_empty() && !line.starts_with("frame=") && !line.starts_with("size="))
        .unwrap_or("FFmpeg exited with an error")
        .to_string()
}

fn run_one(ffmpeg: &Path, payload: &ConvertPayload, input: &Path) -> Result<String, ConvertError> {
    let arguments = args::build(payload, input)?;
    let destination = args::output_path(payload, input)?;

    let output = std::process::Command::new(ffmpeg)
        .args(&arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| ConvertError::Spawn(error.to_string()))?;

    if output.status.success() {
        Ok(destination)
    } else {
        Err(ConvertError::Failed {
            file: input.display().to_string(),
            detail: last_meaningful_line(&String::from_utf8_lossy(&output.stderr)),
        })
    }
}

fn operation_name(operation: Operation) -> String {
    match operation {
        Operation::Convert => "convert",
        Operation::Master => "master",
        Operation::Trim => "trim",
        Operation::Modify => "modify",
        Operation::Analyze => "analyze",
    }
    .to_string()
}

/// Run every file in a job, emitting progress as it goes.
pub fn run(app: &AppHandle, payload: ConvertPayload) -> BackendResult {
    let kind = operation_name(payload.operation);

    if payload.files.is_empty() {
        return BackendResult {
            status: "error".into(),
            message: ConvertError::NoInputs.to_string(),
            data: Vec::new(),
        };
    }

    let ffmpeg = match ffmpeg_path(app) {
        Ok(path) => path,
        Err(error) => {
            let message = error.to_string();
            let _ = app.emit(
                "conversion-progress",
                ProgressEvent::Complete {
                    event: "complete",
                    operation_type: kind,
                    status: "error",
                    message: message.clone(),
                    outputs: Vec::new(),
                },
            );
            return BackendResult {
                status: "error".into(),
                message,
                data: Vec::new(),
            };
        }
    };

    let total = payload.files.len();
    let mut outputs = Vec::with_capacity(total);
    let mut failures = Vec::new();

    for (index, file) in payload.files.iter().enumerate() {
        let _ = app.emit(
            "conversion-progress",
            ProgressEvent::Progress {
                event: "progress",
                operation_type: kind.clone(),
                index: index + 1,
                total,
                file: file.clone(),
                status: "processing",
            },
        );

        match run_one(&ffmpeg, &payload, Path::new(file)) {
            Ok(destination) => outputs.push(destination),
            Err(error) => failures.push(error.to_string()),
        }
    }

    let (status, message) = if failures.is_empty() {
        ("success", format!("Processed {total} file(s)"))
    } else {
        ("error", failures.join("; "))
    };

    let _ = app.emit(
        "conversion-progress",
        ProgressEvent::Complete {
            event: "complete",
            operation_type: kind,
            status,
            message: message.clone(),
            outputs: outputs.clone(),
        },
    );

    BackendResult {
        status: status.into(),
        message,
        data: Vec::new(),
    }
}

/// Measure a file's loudness and suggest a mastering preset.
///
/// The suggestion is a heuristic on integrated loudness alone: quiet, wide
/// material reads as music, loud and narrow reads as speech. It is a starting
/// point for the user, not a classifier, and the UI presents it as such.
pub fn analyze(app: &AppHandle, payload: ConvertPayload) -> BackendResult {
    let Some(file) = payload.files.first().cloned() else {
        return BackendResult {
            status: "error".into(),
            message: ConvertError::NoInputs.to_string(),
            data: Vec::new(),
        };
    };

    let ffmpeg = match ffmpeg_path(app) {
        Ok(path) => path,
        Err(error) => {
            return BackendResult {
                status: "error".into(),
                message: error.to_string(),
                data: Vec::new(),
            }
        }
    };

    // ebur128 writes its summary to stderr and produces no output file.
    let output = std::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-i",
            &file,
            "-filter_complex",
            "ebur128=framelog=quiet",
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null())
        .output();

    let Ok(output) = output else {
        return BackendResult {
            status: "error".into(),
            message: "could not run FFmpeg".into(),
            data: Vec::new(),
        };
    };

    let text = String::from_utf8_lossy(&output.stderr);
    let lufs = parse_integrated_loudness(&text).unwrap_or(-16.0);
    let suggestion = suggest_preset(lufs);

    BackendResult {
        status: "success".into(),
        message: format!("Measured {lufs:.1} LUFS"),
        data: vec![AnalysisEntry {
            file,
            lufs,
            suggestion,
        }],
    }
}

/// Pull the integrated loudness out of an `ebur128` summary.
fn parse_integrated_loudness(text: &str) -> Option<f64> {
    let mut in_summary = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("Integrated loudness") {
            in_summary = true;
            continue;
        }
        if in_summary && line.starts_with("I:") {
            return line
                .trim_start_matches("I:")
                .trim()
                .trim_end_matches("LUFS")
                .trim()
                .parse()
                .ok();
        }
    }
    None
}

/// Map measured loudness onto one of the presets the UI offers.
fn suggest_preset(lufs: f64) -> String {
    // The preset table in MasterControls targets -12 for music, -16 for a
    // podcast and -18 for voice-over. Pick whichever target the material is
    // already closest to.
    if lufs > -14.0 {
        "Music".into()
    } else if lufs > -17.0 {
        "Podcast".into()
    } else {
        "Voice-over".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrated_loudness_is_read_from_an_ebur128_summary() {
        let sample = "\
[Parsed_ebur128_0 @ 000001] Summary:

  Integrated loudness:
    I:         -14.7 LUFS
    Threshold: -25.1 LUFS

  Loudness range:
    LRA:         7.2 LU
";
        assert_eq!(parse_integrated_loudness(sample), Some(-14.7));
    }

    #[test]
    fn a_missing_summary_reads_as_none_rather_than_zero() {
        assert_eq!(parse_integrated_loudness("no summary here"), None);
        // A bare I: outside the summary block must not be picked up.
        assert_eq!(parse_integrated_loudness("I: -9.0 LUFS"), None);
    }

    #[test]
    fn presets_follow_the_targets_the_ui_offers() {
        assert_eq!(suggest_preset(-8.0), "Music");
        assert_eq!(suggest_preset(-13.9), "Music");
        assert_eq!(suggest_preset(-16.0), "Podcast");
        assert_eq!(suggest_preset(-20.0), "Voice-over");
    }

    #[test]
    fn the_missing_ffmpeg_message_matches_what_the_ui_looks_for() {
        // EditorPage decides to show its tools-missing text by searching the
        // message for "located" or "install ffmpeg", case-insensitively.
        let message = ConvertError::FfmpegMissing.to_string().to_lowercase();
        assert!(
            message.contains("located") || message.contains("install ffmpeg"),
            "{message}"
        );
    }

    #[test]
    fn progress_lines_are_skipped_when_reporting_a_failure() {
        let stderr = "\
frame=  100 fps=0.0 q=-1.0 size=     256kB
size=     512kB time=00:00:10.00 bitrate= 419.4kbits/s
Invalid data found when processing input
";
        assert_eq!(
            last_meaningful_line(stderr),
            "Invalid data found when processing input"
        );
    }

    #[test]
    fn an_empty_stderr_still_yields_a_message() {
        assert!(!last_meaningful_line("").is_empty());
        assert!(!last_meaningful_line("\n\n  \n").is_empty());
    }

    #[test]
    fn the_operation_name_matches_the_frontend_vocabulary() {
        // EditorPage filters events on operation_type == "analyze".
        assert_eq!(operation_name(Operation::Analyze), "analyze");
        assert_eq!(operation_name(Operation::Convert), "convert");
        assert_eq!(operation_name(Operation::Master), "master");
        assert_eq!(operation_name(Operation::Trim), "trim");
        assert_eq!(operation_name(Operation::Modify), "modify");
    }

    #[test]
    fn a_payload_from_the_editor_decodes() {
        // Exactly the shape buildPayload posts for each mode, extra fields and
        // all, so a frontend change that adds a field cannot break decoding.
        let raw = r#"{
            "operation": "master",
            "files": ["C:/in/a.wav"],
            "output": "C:/out",
            "concurrent_files": 2,
            "format": "wav",
            "input_paths": ["C:/in/a.wav"],
            "output_directory": "C:/out",
            "preset": "Music",
            "parameters": {
                "target_lufs": -12.0,
                "apply_compression": true,
                "apply_limiter": true,
                "output_gain": 0.0
            }
        }"#;

        let payload: ConvertPayload = serde_json::from_str(raw).expect("should decode");
        assert_eq!(payload.operation, Operation::Master);
        assert_eq!(payload.files.len(), 1);
        assert_eq!(payload.parameters.target_lufs, Some(-12.0));
    }

    #[test]
    fn every_mode_the_editor_can_send_decodes() {
        for operation in ["convert", "master", "trim", "modify", "analyze"] {
            let raw = format!(r#"{{"operation":"{operation}","files":["a.wav"],"output":"out"}}"#);
            let payload: ConvertPayload =
                serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{operation}: {error}"));
            assert_eq!(payload.files, vec!["a.wav".to_string()]);
        }
    }
}
