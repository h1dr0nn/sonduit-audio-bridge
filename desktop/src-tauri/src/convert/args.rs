//! Turning a job description into an FFmpeg argument list.
//!
//! Kept free of process spawning and file system access so the whole mapping
//! can be unit tested without FFmpeg installed. Every function here is pure.

use std::path::Path;

use super::{ConvertError, ConvertPayload, Operation};

/// Codec and container for an output format.
struct Encoding {
    /// FFmpeg encoder name.
    codec: &'static str,
    /// File extension, without the dot.
    extension: &'static str,
    /// Extra encoder arguments, such as a bitrate.
    extra: &'static [&'static str],
}

/// Map a UI format name onto an encoder.
///
/// The UI offers AAC, MP3, WAV, FLAC, OGG and M4A.
fn encoding_for(format: &str) -> Result<Encoding, ConvertError> {
    match format.to_ascii_lowercase().as_str() {
        "aac" => Ok(Encoding {
            codec: "aac",
            extension: "aac",
            extra: &["-b:a", "192k"],
        }),
        "m4a" => Ok(Encoding {
            codec: "aac",
            extension: "m4a",
            extra: &["-b:a", "192k"],
        }),
        "mp3" => Ok(Encoding {
            codec: "libmp3lame",
            extension: "mp3",
            extra: &["-q:a", "2"],
        }),
        "wav" => Ok(Encoding {
            codec: "pcm_s16le",
            extension: "wav",
            extra: &[],
        }),
        "flac" => Ok(Encoding {
            codec: "flac",
            extension: "flac",
            extra: &[],
        }),
        "ogg" => Ok(Encoding {
            codec: "libvorbis",
            extension: "ogg",
            extra: &["-q:a", "5"],
        }),
        other => Err(ConvertError::UnsupportedFormat(other.to_string())),
    }
}

/// The output path for one input, inside the chosen directory.
///
/// Mastering, trimming and modifying keep the input's own extension, because
/// those operations do not change the container the user asked for.
pub fn output_path(payload: &ConvertPayload, input: &Path) -> Result<String, ConvertError> {
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| ConvertError::BadPath(input.display().to_string()))?;

    let extension = match payload.operation {
        Operation::Convert => encoding_for(payload.format.as_deref().unwrap_or("wav"))?
            .extension
            .to_string(),
        _ => input
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("wav")
            .to_string(),
    };

    let suffix = match payload.operation {
        Operation::Convert => "",
        Operation::Master => "_mastered",
        Operation::Trim => "_trimmed",
        Operation::Modify => "_modified",
        Operation::Analyze => "",
    };

    let candidate = Path::new(&payload.output).join(format!("{stem}{suffix}.{extension}"));

    // Converting WAV to WAV into the folder the file already lives in lands on
    // the input itself. FFmpeg refuses that outright ("cannot edit existing
    // files in-place"), so without this the job simply fails for the user.
    if same_file(&candidate, input) {
        return Ok(Path::new(&payload.output)
            .join(format!("{stem}{suffix}_converted.{extension}"))
            .display()
            .to_string());
    }

    Ok(candidate.display().to_string())
}

/// Whether two paths name the same file.
///
/// Compared textually after normalising separators, and case-insensitively on
/// Windows, because the point is to catch a collision before FFmpeg is asked to
/// read and write the same path.
fn same_file(left: &Path, right: &Path) -> bool {
    let normalise = |path: &Path| {
        let text = path.display().to_string().replace('\\', "/");
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    normalise(left) == normalise(right)
}

/// Build the audio filter chain for an operation, or `None` when it needs none.
///
/// Returned as one comma-joined string, which is what `-af` expects.
/// What the first loudnorm pass measured about a file.
///
/// The four values loudnorm itself asks for on a second pass. Their names are
/// FFmpeg's, not chosen here, because they are passed straight back to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessMeasurement {
    /// Integrated loudness, LUFS.
    pub input_i: f64,
    /// True peak, dBTP.
    pub input_tp: f64,
    /// Loudness range, LU.
    pub input_lra: f64,
    /// Measured threshold, LUFS.
    pub input_thresh: f64,
    /// Offset loudnorm computed for itself.
    pub target_offset: f64,
}

/// Loudness range the mastering chain targets, in LU.
///
/// The EBU R128 broadcast figure, and the value loudnorm is given in both
/// passes. It also decides whether a measured file can be normalised with a
/// single gain at all: see [`loudnorm`].
pub const TARGET_LRA: f64 = 11.0;

/// Build the loudnorm filter, with the measurement from a first pass if there
/// is one.
///
/// # Why two passes
///
/// In one pass loudnorm is working blind: it normalises against a running
/// estimate that is wrong at the start of the file and only converges later,
/// so the opening sits at a different level from the rest and the integrated
/// result misses the target. Handing back what a measurement pass found lets
/// it apply a single exact gain instead.
///
/// Measured against the bundled FFmpeg on a file with a 6 LU range and a
/// -14 LUFS target: one pass lands at -11.97, two passes at -14.03.
///
/// # Why a wide file goes back to one pass
///
/// `linear=true` is a request, not a guarantee. When the measured range
/// exceeds the target range, one gain cannot reach the target, and loudnorm
/// silently falls back to its dynamic mode. On the same test with a 21 LU
/// range that fallback landed at -12.50 while the single pass landed at
/// -13.98, so the second pass made the result worse.
///
/// So the measurement is used when it can be used linearly, and discarded
/// otherwise. Discarding it is not a loss: the single pass is what would have
/// run anyway.
///
/// The measurement is optional because it costs a full decode of the file. A
/// caller that wants a preview rather than a master can skip it.
#[must_use]
pub fn loudnorm(payload: &ConvertPayload, measured: Option<LoudnessMeasurement>) -> String {
    let target = payload.parameters.target_lufs.unwrap_or(-14.0);
    let base = format!("loudnorm=I={target}:TP=-1.5:LRA={TARGET_LRA}");

    let Some(m) = measured else {
        return base;
    };
    if !m.input_lra.is_finite() || m.input_lra > TARGET_LRA {
        return base;
    }

    // linear=true asks for one constant gain rather than the dynamic mode.
    // With a measurement, and a range that fits, that is both exact and
    // transparent.
    let mut filter = base;
    filter.push_str(&format!(":measured_I={:.2}", m.input_i));
    filter.push_str(&format!(":measured_TP={:.2}", m.input_tp));
    filter.push_str(&format!(":measured_LRA={:.2}", m.input_lra));
    filter.push_str(&format!(":measured_thresh={:.2}", m.input_thresh));
    filter.push_str(&format!(":offset={:.2}", m.target_offset));
    filter.push_str(":linear=true");
    filter
}

pub fn filter_chain(
    payload: &ConvertPayload,
    measured: Option<LoudnessMeasurement>,
) -> Option<String> {
    let mut filters: Vec<String> = Vec::new();

    match payload.operation {
        Operation::Convert | Operation::Analyze => {}

        Operation::Master => {
            filters.push(loudnorm(payload, measured));

            if payload.parameters.apply_compression.unwrap_or(false) {
                filters.push("acompressor=threshold=-18dB:ratio=3:attack=20:release=250".into());
            }
            if payload.parameters.apply_limiter.unwrap_or(false) {
                filters.push("alimiter=limit=0.98".into());
            }

            let gain = payload.parameters.output_gain.unwrap_or(0.0);
            if gain != 0.0 {
                filters.push(format!("volume={gain}dB"));
            }
        }

        Operation::Trim => {
            let threshold = payload.silence_threshold.unwrap_or(-50.0);
            let minimum_ms = payload.minimum_silence_ms.unwrap_or(500).max(1);
            let padding_ms = payload.padding_ms.unwrap_or(100);

            // Silence shorter than the minimum is left alone. The padding the
            // user asked to keep is expressed by shortening the detection
            // window, since silenceremove has no separate padding control.
            let keep_seconds = f64::from(padding_ms) / 1000.0;
            let window_seconds = (f64::from(minimum_ms) / 1000.0 - keep_seconds).max(0.01);

            filters.push(format!(
                "silenceremove=start_periods=1:start_duration={window_seconds}:start_threshold={threshold}dB:\
detection=peak,areverse,\
silenceremove=start_periods=1:start_duration={window_seconds}:start_threshold={threshold}dB:\
detection=peak,areverse"
            ));
        }

        Operation::Modify => {
            let speed = payload.speed.unwrap_or(1.0);
            if (speed - 1.0).abs() > f64::EPSILON {
                // atempo only accepts 0.5..=2.0, so a larger change is chained.
                for factor in split_tempo(speed) {
                    filters.push(format!("atempo={factor}"));
                }
            }

            let pitch = payload.pitch.unwrap_or(0.0);
            if pitch.abs() > f64::EPSILON {
                // Resample to shift pitch, then restore the duration that the
                // resample changed. Semitones are a ratio of 2^(n/12).
                let ratio = 2_f64.powf(pitch / 12.0);
                filters.push(format!("asetrate=48000*{ratio}"));
                filters.push("aresample=48000".into());
                for factor in split_tempo(1.0 / ratio) {
                    filters.push(format!("atempo={factor}"));
                }
            }
        }
    }

    if filters.is_empty() {
        None
    } else {
        Some(filters.join(","))
    }
}

/// Arguments for the measurement pass that precedes a master.
///
/// Decodes the whole file, runs loudnorm in analysis mode and discards the
/// output. Only the JSON it prints to stderr is wanted.
#[must_use]
pub fn measure_args(payload: &ConvertPayload, input: &Path) -> Vec<String> {
    let target = payload.parameters.target_lufs.unwrap_or(-14.0);
    vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-i".into(),
        input.display().to_string(),
        "-af".into(),
        format!("loudnorm=I={target}:TP=-1.5:LRA={TARGET_LRA}:print_format=json"),
        "-f".into(),
        "null".into(),
        // Not the platform null device: FFmpeg's own null muxer takes a path
        // it ignores, and "-" works on every platform where NUL and /dev/null
        // do not.
        "-".into(),
    ]
}

/// Split a tempo ratio into factors FFmpeg's `atempo` will accept.
///
/// `atempo` is limited to 0.5..=2.0 per instance, so anything beyond that has
/// to be applied as a chain.
fn split_tempo(mut ratio: f64) -> Vec<f64> {
    let mut factors = Vec::new();
    if ratio <= 0.0 {
        return factors;
    }

    while ratio > 2.0 {
        factors.push(2.0);
        ratio /= 2.0;
    }
    while ratio < 0.5 {
        factors.push(0.5);
        ratio /= 0.5;
    }
    if (ratio - 1.0).abs() > f64::EPSILON {
        factors.push((ratio * 1e6).round() / 1e6);
    }
    factors
}

/// Full argument list for one input file.
///
/// # Errors
/// Propagates format and path failures.
pub fn build(
    payload: &ConvertPayload,
    input: &Path,
    measured: Option<LoudnessMeasurement>,
) -> Result<Vec<String>, ConvertError> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-y".into()];

    // A cut is expressed as input seeking so FFmpeg never decodes what it is
    // going to throw away.
    if payload.operation == Operation::Modify && payload.is_cut_enabled.unwrap_or(false) {
        if let Some(start) = payload.cut_start {
            if start > 0.0 {
                args.push("-ss".into());
                args.push(format!("{start}"));
            }
        }
    }

    args.push("-i".into());
    args.push(input.display().to_string());

    if payload.operation == Operation::Modify && payload.is_cut_enabled.unwrap_or(false) {
        if let Some(end) = payload.cut_end {
            let start = payload.cut_start.unwrap_or(0.0);
            if end > start {
                args.push("-t".into());
                args.push(format!("{}", end - start));
            }
        }
    }

    if let Some(chain) = filter_chain(payload, measured) {
        args.push("-af".into());
        args.push(chain);
    }

    match payload.operation {
        Operation::Convert => {
            let encoding = encoding_for(payload.format.as_deref().unwrap_or("wav"))?;
            args.push("-c:a".into());
            args.push(encoding.codec.into());
            args.extend(encoding.extra.iter().map(|value| (*value).to_string()));
        }
        _ => {
            // Filters produce new samples, so the stream cannot be copied.
            // Re-encode into whatever the container implies.
            args.push("-c:a".into());
            args.push(
                encoding_for(
                    input
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("wav"),
                )
                .map(|encoding| encoding.codec.to_string())
                .unwrap_or_else(|_| "pcm_s16le".to_string()),
            );
        }
    }

    args.push(output_path(payload, input)?);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::MasterParameters;

    fn payload(operation: Operation) -> ConvertPayload {
        ConvertPayload {
            operation,
            files: vec!["C:/in/song.wav".into()],
            output: "C:/out".into(),
            format: Some("mp3".into()),
            parameters: MasterParameters::default(),
            silence_threshold: None,
            minimum_silence_ms: None,
            padding_ms: None,
            speed: None,
            pitch: None,
            cut_start: None,
            cut_end: None,
            is_cut_enabled: None,
        }
    }

    #[test]
    fn convert_picks_the_encoder_and_extension_for_each_format() {
        let cases = [
            ("mp3", "libmp3lame", "song.mp3"),
            ("wav", "pcm_s16le", "song.wav"),
            ("flac", "flac", "song.flac"),
            ("aac", "aac", "song.aac"),
            ("m4a", "aac", "song.m4a"),
            ("ogg", "libvorbis", "song.ogg"),
        ];

        for (format, codec, filename) in cases {
            let mut job = payload(Operation::Convert);
            job.format = Some(format.into());
            let args = build(&job, Path::new("C:/in/song.wav"), None).unwrap();

            assert!(
                args.contains(&codec.to_string()),
                "{format} should use {codec}"
            );
            assert!(
                args.last().unwrap().ends_with(filename),
                "{format} should write {filename}, got {}",
                args.last().unwrap()
            );
        }
    }

    #[test]
    fn an_unknown_format_is_refused_rather_than_guessed() {
        let mut job = payload(Operation::Convert);
        job.format = Some("wma".into());
        assert!(matches!(
            build(&job, Path::new("C:/in/song.wav"), None),
            Err(ConvertError::UnsupportedFormat(_))
        ));
    }

    fn measurement(lra: f64) -> LoudnessMeasurement {
        LoudnessMeasurement {
            input_i: -22.19,
            input_tp: -6.85,
            input_lra: lra,
            input_thresh: -32.20,
            target_offset: -0.01,
        }
    }

    #[test]
    fn a_measurement_that_fits_the_target_range_is_used_linearly() {
        // The accurate path. Verified against the bundled FFmpeg: -14.03 LUFS
        // against a -14 target, where one pass reached -11.97.
        let job = payload(Operation::Master);
        let filter = loudnorm(&job, Some(measurement(6.30)));

        assert!(filter.contains("measured_I=-22.19"), "{filter}");
        assert!(filter.contains("measured_LRA=6.30"), "{filter}");
        assert!(filter.contains("linear=true"), "{filter}");
    }

    #[test]
    fn a_file_wider_than_the_target_range_falls_back_to_one_pass() {
        // linear=true is a request, not a guarantee: loudnorm drops to its
        // dynamic mode when one gain cannot reach the target, and on the same
        // test that fallback landed 1.5 LU further off than the single pass.
        let job = payload(Operation::Master);
        let filter = loudnorm(&job, Some(measurement(TARGET_LRA + 0.1)));

        assert!(!filter.contains("measured_I"), "{filter}");
        assert!(!filter.contains("linear"), "{filter}");
    }

    #[test]
    fn a_range_exactly_at_the_target_is_still_used() {
        // The boundary is inclusive: a file whose range equals the target can
        // be normalised with one gain.
        let job = payload(Operation::Master);
        let filter = loudnorm(&job, Some(measurement(TARGET_LRA)));
        assert!(filter.contains("linear=true"), "{filter}");
    }

    #[test]
    fn a_non_finite_range_falls_back_rather_than_being_formatted() {
        let job = payload(Operation::Master);
        let filter = loudnorm(&job, Some(measurement(f64::NAN)));
        assert!(!filter.contains("measured_LRA"), "{filter}");
    }

    #[test]
    fn the_measurement_pass_asks_for_json_and_writes_nothing() {
        let job = payload(Operation::Master);
        let args = measure_args(&job, Path::new("C:/in/song.wav"));

        assert!(
            args.iter().any(|a| a.contains("print_format=json")),
            "{args:?}"
        );
        assert!(
            args.windows(2).any(|w| w[0] == "-f" && w[1] == "null"),
            "{args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|a| a.ends_with(".wav") && a.starts_with("C:/out")),
            "the measurement pass must not write an output file: {args:?}"
        );
    }

    #[test]
    fn master_builds_a_loudnorm_chain_from_the_preset() {
        let mut job = payload(Operation::Master);
        job.parameters = MasterParameters {
            target_lufs: Some(-12.0),
            apply_compression: Some(true),
            apply_limiter: Some(true),
            output_gain: Some(1.5),
        };

        let chain = filter_chain(&job, None).expect("master needs filters");
        assert!(chain.contains("loudnorm=I=-12"), "{chain}");
        assert!(chain.contains("acompressor"), "{chain}");
        assert!(chain.contains("alimiter"), "{chain}");
        assert!(chain.contains("volume=1.5dB"), "{chain}");
    }

    #[test]
    fn master_omits_the_stages_the_preset_turned_off() {
        let mut job = payload(Operation::Master);
        job.parameters = MasterParameters {
            target_lufs: Some(-16.0),
            apply_compression: Some(false),
            apply_limiter: Some(false),
            output_gain: Some(0.0),
        };

        let chain = filter_chain(&job, None).expect("master needs filters");
        assert!(!chain.contains("acompressor"), "{chain}");
        assert!(!chain.contains("alimiter"), "{chain}");
        assert!(!chain.contains("volume="), "{chain}");
    }

    #[test]
    fn mastering_keeps_the_input_extension() {
        let job = payload(Operation::Master);
        let path = output_path(&job, Path::new("C:/in/song.flac")).unwrap();
        assert!(path.ends_with("song_mastered.flac"), "{path}");
    }

    #[test]
    fn each_operation_gets_its_own_suffix() {
        for (operation, suffix) in [
            (Operation::Master, "_mastered"),
            (Operation::Trim, "_trimmed"),
            (Operation::Modify, "_modified"),
        ] {
            let job = payload(operation);
            let path = output_path(&job, Path::new("C:/in/song.wav")).unwrap();
            assert!(path.contains(suffix), "{operation:?} -> {path}");
        }
    }

    #[test]
    fn an_output_that_would_overwrite_its_input_is_renamed() {
        // Converting WAV to WAV into the source folder lands on the input.
        // FFmpeg refuses to edit a file in place, so the job would just fail.
        let mut job = payload(Operation::Convert);
        job.format = Some("wav".into());
        job.output = "C:/in".into();

        let path = output_path(&job, Path::new("C:/in/song.wav")).unwrap();
        assert_ne!(path.replace('\\', "/"), "C:/in/song.wav");
        assert!(path.ends_with("song_converted.wav"), "{path}");
    }

    #[test]
    fn a_collision_is_detected_regardless_of_separator_or_case() {
        let mut job = payload(Operation::Convert);
        job.format = Some("wav".into());
        job.output = r"C:\In".into();

        let path = output_path(&job, Path::new("C:/in/Song.wav")).unwrap();
        assert!(path.ends_with("Song_converted.wav"), "{path}");
    }

    #[test]
    fn a_different_folder_keeps_the_plain_name() {
        let mut job = payload(Operation::Convert);
        job.format = Some("wav".into());
        job.output = "C:/out".into();

        let path = output_path(&job, Path::new("C:/in/song.wav")).unwrap();
        assert!(path.ends_with("song.wav"), "{path}");
        assert!(!path.contains("_converted"), "{path}");
    }

    #[test]
    fn convert_does_not_suffix_the_name() {
        let job = payload(Operation::Convert);
        let path = output_path(&job, Path::new("C:/in/song.wav")).unwrap();
        assert!(path.ends_with("song.mp3"), "{path}");
        assert!(!path.contains("_convert"), "{path}");
    }

    #[test]
    fn trim_builds_a_symmetric_silenceremove() {
        let mut job = payload(Operation::Trim);
        job.silence_threshold = Some(-45.0);
        job.minimum_silence_ms = Some(800);
        job.padding_ms = Some(100);

        let chain = filter_chain(&job, None).expect("trim needs filters");
        // Once for the head, once for the tail via areverse.
        assert_eq!(chain.matches("silenceremove").count(), 2, "{chain}");
        assert_eq!(chain.matches("areverse").count(), 2, "{chain}");
        assert!(chain.contains("start_threshold=-45dB"), "{chain}");
    }

    #[test]
    fn trim_never_produces_a_negative_window() {
        // Padding larger than the minimum silence would otherwise go negative.
        let mut job = payload(Operation::Trim);
        job.minimum_silence_ms = Some(100);
        job.padding_ms = Some(900);

        let chain = filter_chain(&job, None).expect("trim needs filters");

        // Check the duration itself, not the whole string: the threshold is
        // legitimately negative ("-50dB") and a naive dash search catches it.
        for field in chain.split(':') {
            if let Some(value) = field.strip_prefix("start_duration=") {
                let seconds: f64 = value.parse().expect("duration should be a number");
                assert!(
                    seconds > 0.0,
                    "start_duration must stay positive, got {seconds}"
                );
            }
        }
    }

    #[test]
    fn tempo_is_split_into_factors_ffmpeg_accepts() {
        for ratio in [0.5, 0.75, 1.5, 2.0, 3.0, 4.0, 0.25, 0.3] {
            let factors = split_tempo(ratio);
            for factor in &factors {
                assert!(
                    (0.5..=2.0).contains(factor),
                    "atempo={factor} is outside the range ffmpeg allows (from {ratio})"
                );
            }
            let product: f64 = factors.iter().product();
            assert!(
                (product - ratio).abs() < 1e-4,
                "factors {factors:?} multiply to {product}, wanted {ratio}"
            );
        }
    }

    #[test]
    fn a_tempo_of_one_produces_no_filter() {
        assert!(split_tempo(1.0).is_empty());
        let mut job = payload(Operation::Modify);
        job.speed = Some(1.0);
        job.pitch = Some(0.0);
        assert!(filter_chain(&job, None).is_none());
    }

    #[test]
    fn a_pitch_shift_restores_the_original_duration() {
        let mut job = payload(Operation::Modify);
        job.pitch = Some(12.0); // one octave up
        let chain = filter_chain(&job, None).expect("pitch needs filters");

        assert!(chain.contains("asetrate"), "{chain}");
        assert!(chain.contains("aresample"), "{chain}");
        // Doubling the rate halves the duration, so tempo must halve it back.
        assert!(chain.contains("atempo=0.5"), "{chain}");
    }

    #[test]
    fn a_cut_seeks_on_the_input_and_bounds_the_duration() {
        let mut job = payload(Operation::Modify);
        job.is_cut_enabled = Some(true);
        job.cut_start = Some(10.0);
        job.cut_end = Some(25.0);

        let args = build(&job, Path::new("C:/in/song.wav"), None).unwrap();
        let seek = args.iter().position(|a| a == "-ss").expect("-ss");
        let input = args.iter().position(|a| a == "-i").expect("-i");

        assert!(
            seek < input,
            "-ss must precede -i so the skip is not decoded"
        );
        assert_eq!(args[seek + 1], "10");
        let duration = args.iter().position(|a| a == "-t").expect("-t");
        assert_eq!(args[duration + 1], "15");
    }

    #[test]
    fn a_cut_that_is_switched_off_is_ignored() {
        let mut job = payload(Operation::Modify);
        job.is_cut_enabled = Some(false);
        job.cut_start = Some(10.0);
        job.cut_end = Some(25.0);

        let args = build(&job, Path::new("C:/in/song.wav"), None).unwrap();
        assert!(!args.contains(&"-ss".to_string()));
        assert!(!args.contains(&"-t".to_string()));
    }

    #[test]
    fn every_job_overwrites_and_never_waits_on_stdin() {
        // Without -nostdin ffmpeg can block forever reading the parent stdin,
        // which in a GUI process never produces anything.
        for operation in [
            Operation::Convert,
            Operation::Master,
            Operation::Trim,
            Operation::Modify,
        ] {
            let args = build(&payload(operation), Path::new("C:/in/song.wav"), None).unwrap();
            assert!(args.contains(&"-nostdin".to_string()), "{operation:?}");
            assert!(args.contains(&"-y".to_string()), "{operation:?}");
        }
    }
}
