//! One way to run an ffmpeg child process: spawn, register it for
//! cancellation, stream its events, then check exit status and output.

use crate::format_ffmpeg_spawn_error;
use crate::run_control::{RunControl, RUN_CANCELLED_MESSAGE};
use crate::time_utils::parse_timestamp_to_seconds_raw;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::{FfmpegEvent, LogLevel};
use log::debug;
use std::path::Path;

/// Describes one ffmpeg run for cancellation and error reporting.
pub struct FfmpegTask<'a> {
    /// What the run does, phrased to follow "Failed to", e.g. "remove silence".
    pub operation: &'a str,
    /// The main input, named in error messages.
    pub input: &'a Path,
    /// The file the run must produce. Runs that only analyse (`-f null -`)
    /// leave this unset.
    pub output: Option<&'a Path>,
    /// The run this process belongs to, so cancelling the run kills it.
    pub run: Option<(u64, &'a RunControl)>,
}

/// Run `command` to completion, passing every event to `on_event`.
///
/// Succeeds only if the run was not cancelled, ffmpeg exited successfully and
/// the expected output exists. The exit status matters: with `-y`, an output
/// left over from an earlier run still exists when this run fails before
/// opening it. Errors are messages for the UI; cancellation yields
/// [`RUN_CANCELLED_MESSAGE`].
///
/// This blocks until ffmpeg exits, so call it from a blocking context
/// (`tokio::task::spawn_blocking`), not directly on the async runtime.
pub fn run_ffmpeg(
    mut command: FfmpegCommand,
    task: FfmpegTask<'_>,
    mut on_event: impl FnMut(&FfmpegEvent),
) -> Result<(), String> {
    if let Some((run_id, run_control)) = task.run {
        run_control.ensure_active(run_id)?;
    }

    let mut child = command
        .spawn()
        .map_err(|e| format_ffmpeg_spawn_error(task.operation, task.input, task.output, &e))?;
    let pid = child.as_inner().id();

    if let Some((run_id, run_control)) = task.run {
        // register_pid kills the process itself if the run was cancelled in
        // the meantime; reap it before bailing out.
        if let Err(error) = run_control.register_pid(run_id, pid) {
            let _ = child.wait();
            return Err(error);
        }
    }

    let mut last_error = None;
    match child.iter() {
        Ok(events) => {
            for event in events {
                match &event {
                    FfmpegEvent::Error(message)
                    | FfmpegEvent::Log(LogLevel::Error | LogLevel::Fatal, message) => {
                        last_error = Some(message.clone());
                    }
                    FfmpegEvent::Log(_, message) => debug!("[ffmpeg] {message}"),
                    _ => {}
                }
                on_event(&event);
            }
        }
        Err(error) => {
            last_error = Some(format!("Failed to read ffmpeg output: {error}"));
            let _ = child.kill();
        }
    }
    let status = child.wait();

    if let Some((run_id, run_control)) = task.run {
        run_control.clear_pid(run_id, pid);
        if run_control.is_cancelled(run_id) {
            return Err(RUN_CANCELLED_MESSAGE.to_string());
        }
    }

    let exited_successfully = matches!(&status, Ok(status) if status.success());
    let output_missing = task.output.is_some_and(|output| !output.exists());
    if exited_successfully && !output_missing {
        return Ok(());
    }

    let detail = last_error.unwrap_or_else(|| match &status {
        Ok(status) if !status.success() => format!("ffmpeg exited with {status}"),
        Ok(_) => "ffmpeg finished without creating the output".to_string(),
        Err(error) => format!("Failed to wait for ffmpeg: {error}"),
    });
    Err(match task.output {
        Some(output) => format!(
            "Failed to {} for '{}' -> '{}': {}",
            task.operation,
            task.input.display(),
            output.display(),
            detail
        ),
        None => format!(
            "Failed to {} for '{}': {}",
            task.operation,
            task.input.display(),
            detail
        ),
    })
}

/// Progress through a media file of `total_seconds`, from an ffmpeg `time=`
/// value, as a percentage. `None` when the total is unknown.
pub fn progress_percentage(time: &str, total_seconds: Option<f64>) -> Option<f64> {
    let total = total_seconds.filter(|total| *total > 0.0)?;
    let current = parse_timestamp_to_seconds_raw(time).unwrap_or(0.0);
    Some((current / total * 100.0).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn lavfi_tone(seconds: u32) -> FfmpegCommand {
        let mut command = FfmpegCommand::new();
        command
            .args(["-f", "lavfi"])
            .input(format!("sine=frequency=440:duration={seconds}"));
        command
    }

    #[test]
    fn succeeds_when_ffmpeg_writes_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("tone.m4a");
        let mut command = lavfi_tone(1);
        command
            .args(["-y", "-c:a", "aac"])
            .output(output.to_str().unwrap());

        let mut saw_progress = false;
        run_ffmpeg(
            command,
            FfmpegTask {
                operation: "write a tone",
                input: Path::new("lavfi"),
                output: Some(&output),
                run: None,
            },
            |event| saw_progress |= matches!(event, FfmpegEvent::Progress(_)),
        )
        .unwrap();
        assert!(output.exists());
        assert!(saw_progress);
    }

    #[test]
    fn a_stale_output_does_not_hide_a_failed_run() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("stale.m4a");
        std::fs::write(&output, b"left over from an earlier run").unwrap();
        let missing_input = dir.path().join("missing.wav");
        let mut command = FfmpegCommand::new();
        command
            .input(missing_input.to_str().unwrap())
            .args(["-y"])
            .output(output.to_str().unwrap());

        let error = run_ffmpeg(
            command,
            FfmpegTask {
                operation: "convert audio",
                input: &missing_input,
                output: Some(&output),
                run: None,
            },
            |_| {},
        )
        .unwrap_err();
        assert!(error.starts_with("Failed to convert audio for"), "{error}");
    }

    #[test]
    fn cancelling_the_run_kills_ffmpeg_and_reports_cancellation() {
        let control = RunControl::default();
        let run_id = control.begin_run();
        // A minute of audio to /dev/null: long enough to still be running
        // when the run is cancelled.
        // `-re` paces the input at real time.
        let mut command = FfmpegCommand::new();
        command
            .args(["-re", "-f", "lavfi"])
            .input("sine=frequency=440:duration=60")
            .args(["-f", "null", "-"]);

        let canceller = control.clone();
        let cancel_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            canceller.cancel_current_run().unwrap();
        });

        let started = std::time::Instant::now();
        let error = run_ffmpeg(
            command,
            FfmpegTask {
                operation: "play a tone",
                input: Path::new("lavfi"),
                output: None,
                run: Some((run_id, &control)),
            },
            |_| {},
        )
        .unwrap_err();
        cancel_thread.join().unwrap();

        assert_eq!(error, RUN_CANCELLED_MESSAGE);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn progress_is_a_clamped_percentage_of_a_known_total() {
        assert_eq!(progress_percentage("00:00:05.00", Some(10.0)), Some(50.0));
        assert_eq!(progress_percentage("00:00:12.00", Some(10.0)), Some(100.0));
        assert_eq!(progress_percentage("00:00:05.00", None), None);
        assert_eq!(progress_percentage("00:00:05.00", Some(0.0)), None);
    }
}
