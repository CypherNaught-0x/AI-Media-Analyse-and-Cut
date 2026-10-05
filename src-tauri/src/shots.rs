//! Hard cuts in the source (shorts phase S2): face tracks and the virtual
//! camera reset at every cut, so a crop never pans across one.

use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use crate::run_control::RunControl;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

/// `scdet` score (0-100) above which a frame starts a new shot. 10 is the
/// filter's default; it catches hard cuts but not camera pans or fades.
const SCENE_THRESHOLD: f64 = 10.0;
/// Detection runs on a small copy; cuts don't need detail.
const ANALYSIS_WIDTH: u32 = 320;

static CUT_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"lavfi\.scd\.time=(\d+(?:\.\d+)?)").unwrap());

/// Source times (seconds) where a new shot starts within `start..end`.
pub(crate) fn detect_cuts(
    path: &Path,
    start: f64,
    end: f64,
    run: Option<(u64, &RunControl)>,
) -> Result<Vec<f64>, String> {
    if end <= start {
        return Ok(Vec::new());
    }
    let mut command = FfmpegCommand::new();
    command
        .args(["-ss", &format!("{start:.6}")])
        .input(path.to_string_lossy())
        .args(["-t", &format!("{:.6}", end - start)])
        .args(["-an", "-sn"])
        .filter(format!(
            "scale={ANALYSIS_WIDTH}:-2,scdet=threshold={SCENE_THRESHOLD}:sc_pass=1,\
             metadata=print:key=lavfi.scd.time"
        ))
        .args(["-f", "null", "-"]);

    let mut cuts = Vec::new();
    run_ffmpeg(
        command,
        FfmpegTask {
            operation: "detect shot changes",
            input: path,
            output: None,
            run,
        },
        |event| {
            if let FfmpegEvent::Log(_, line) = event {
                if let Some(time) = CUT_TIME
                    .captures(line)
                    .and_then(|captures| captures[1].parse::<f64>().ok())
                {
                    // Times are relative to the input seek.
                    cuts.push(start + time);
                }
            }
        },
    )?;
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    Ok(cuts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::tests::colour_video;

    #[test]
    fn finds_the_hard_cuts_between_shots() {
        let dir = tempfile::tempdir().unwrap();
        // Distinct brightness: scdet compares mostly luma (pure red and green
        // are nearly equal there).
        let video = colour_video(dir.path(), &["black", "white", "gray", "navy"]);

        let cuts = detect_cuts(&video, 0.0, 4.0, None).unwrap();
        assert_eq!(cuts.len(), 3, "{cuts:?}");
        for (cut, expected) in cuts.iter().zip([1.0, 2.0, 3.0]) {
            assert!((cut - expected).abs() < 0.05, "{cuts:?}");
        }

        // A range that starts mid-video reports source times.
        let cuts = detect_cuts(&video, 1.5, 4.0, None).unwrap();
        assert_eq!(cuts.len(), 2, "{cuts:?}");
        assert!((cuts[0] - 2.0).abs() < 0.05, "{cuts:?}");
    }
}
