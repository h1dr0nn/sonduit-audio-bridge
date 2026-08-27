//! Does a master built by this crate land on its target?
//!
//! Runs the arguments `convert::args` produces against the bundled FFmpeg and
//! measures the result. The two-pass logic is only worth having if the number
//! it produces is better, and the only way to know is to look.
//!
//! Run with `cargo run -p sonduit-desktop --example master_check -- <ffmpeg>`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sonduit_desktop::convert::args::{self, LoudnessMeasurement};
use sonduit_desktop::convert::{ConvertPayload, MasterParameters, Operation};

fn ffmpeg() -> PathBuf {
    std::env::args().nth(1).map_or_else(
        || PathBuf::from("desktop/src-tauri/binaries/ffmpeg.exe"),
        PathBuf::from,
    )
}

/// Build a file whose level changes part way through, which is what defeats a
/// single pass: it normalises against the opening and the rest sits wrong.
fn source(ffmpeg: &Path, path: &Path, quiet_db: i32, loud_db: i32) {
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=220:duration=6",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=6",
            "-filter_complex",
            &format!(
                "[0:a]volume={quiet_db}dB[a];[1:a]volume={loud_db}dB[b];[a][b]concat=n=2:v=0:a=1"
            ),
            "-ar",
            "48000",
            "-ac",
            "2",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .expect("ffmpeg should run");
    assert!(status.success(), "could not build the source file");
}

fn measure(ffmpeg: &Path, payload: &ConvertPayload, path: &Path) -> Option<LoudnessMeasurement> {
    let output = Command::new(ffmpeg)
        .args(args::measure_args(payload, path))
        .stdin(Stdio::null())
        .output()
        .ok()?;
    parse(&String::from_utf8_lossy(&output.stderr))
}

fn parse(text: &str) -> Option<LoudnessMeasurement> {
    fn field(text: &str, name: &str) -> Option<f64> {
        let key = format!("\"{name}\"");
        let at = text.find(&key)?;
        let rest = &text[at + key.len()..];
        let start = rest.find('"')? + 1;
        let end = start + rest[start..].find('"')?;
        rest[start..end]
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    }

    Some(LoudnessMeasurement {
        input_i: field(text, "input_i")?,
        input_tp: field(text, "input_tp")?,
        input_lra: field(text, "input_lra")?,
        input_thresh: field(text, "input_thresh")?,
        target_offset: field(text, "target_offset")?,
    })
}

fn run(
    ffmpeg: &Path,
    payload: &ConvertPayload,
    input: &Path,
    measured: Option<LoudnessMeasurement>,
) {
    let arguments = args::build(payload, input, measured).expect("arguments should build");
    let output = Command::new(ffmpeg)
        .args(&arguments)
        .stdin(Stdio::null())
        .output()
        .expect("ffmpeg should run");
    assert!(
        output.status.success(),
        "master failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn main() {
    let ffmpeg = ffmpeg();
    let dir = std::env::temp_dir().join("sonduit-master-check");
    std::fs::create_dir_all(&dir).unwrap();

    let target = -14.0;

    for (label, quiet, loud) in [("narrow", -20, -14), ("wide", -24, -3)] {
        let input = dir.join(format!("{label}.wav"));
        source(&ffmpeg, &input, quiet, loud);

        let mut payload = ConvertPayload {
            files: vec![input.display().to_string()],
            output: dir.display().to_string(),
            operation: Operation::Master,
            format: Some("wav".into()),
            ..ConvertPayload::default()
        };
        payload.parameters = MasterParameters {
            target_lufs: Some(target),
            ..MasterParameters::default()
        };

        let measured = measure(&ffmpeg, &payload, &input).expect("measurement should parse");
        run(&ffmpeg, &payload, &input, Some(measured));

        let produced = args::output_path(&payload, &input).unwrap();
        let after = measure(&ffmpeg, &payload, Path::new(&produced))
            .expect("the result should measure")
            .input_i;

        let linear = args::loudnorm(&payload, Some(measured)).contains("linear=true");
        println!(
            "{label:7} lra {:5.2}  {}  -> {after:6.2} LUFS  (target {target})",
            measured.input_lra,
            if linear { "two pass" } else { "one pass" },
        );

        let error = (after - target).abs();
        println!("        error {error:.2} LU");
        assert!(
            error < 1.0,
            "{label} landed {error:.2} LU from the target, which is worse than one pass"
        );
    }

    println!("RESULT: ok");
}
