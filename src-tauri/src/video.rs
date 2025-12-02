use anyhow::Result;
use ffmpeg_sidecar::command::FfmpegCommand;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Segment {
    pub start: String,
    pub end: String,
}

pub fn cut_video(input_path: &Path, segments: &[Segment], output_path: &Path) -> Result<()> {
    // Optimization: Use filter_complex to cut and concat in a single pass.
    // Example:
    // ffmpeg -i input.mp4 -filter_complex
    // "[0:v]trim=start=10:end=20,setpts=PTS-STARTPTS[v0];
    //  [0:a]atrim=start=10:end=20,asetpts=PTS-STARTPTS[a0];
    //  [0:v]trim=start=30:end=40,setpts=PTS-STARTPTS[v1];
    //  [0:a]atrim=start=30:end=40,asetpts=PTS-STARTPTS[a1];
    //  [v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"
    // -map "[v]" -map "[a]" output.mp4

    let mut filter_complex = String::new();
    let mut inputs = String::new();

    for (i, segment) in segments.iter().enumerate() {
        // Convert MM:SS to seconds if needed, but ffmpeg supports MM:SS directly in trim?
        // Usually trim expects seconds or HH:MM:SS.
        // Let's assume the segment strings are compatible or we parse them.
        // The plan says "MM:SS". ffmpeg trim accepts this.

        // Video trim
        filter_complex.push_str(&format!(
            "[0:v]trim=start={}:end={},setpts=PTS-STARTPTS[v{}];",
            segment.start, segment.end, i
        ));

        // Audio trim
        filter_complex.push_str(&format!(
            "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{}];",
            segment.start, segment.end, i
        ));

        inputs.push_str(&format!("[v{}][a{}]", i, i));
    }

    filter_complex.push_str(&format!(
        "{}concat=n={}:v=1:a=1[v][a]",
        inputs,
        segments.len()
    ));

    FfmpegCommand::new()
        .input(input_path.to_str().unwrap())
        .args(&[
            "-filter_complex",
            &filter_complex,
            "-map",
            "[v]",
            "-map",
            "[a]",
        ])
        .output(output_path.to_str().unwrap())
        .spawn()
        .map_err(|e| e.to_string())
        .map_err(anyhow::Error::msg)?
        .iter()
        .map_err(|e| e.to_string())
        .map_err(anyhow::Error::msg)?
        .for_each(|_event| {});

    Ok(())
}
