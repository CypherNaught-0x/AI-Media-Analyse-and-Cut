use anyhow::Result;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Segment {
    pub start: String,
    pub end: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ClipSegment {
    pub start: String,
    pub end: String,
    pub label: Option<String>,
}

pub fn cut_video<F>(
    input_path: &Path,
    segments: &[Segment],
    output_path: &Path,
    on_progress: F,
) -> Result<()>
where
    F: Fn(String) + Send + 'static,
{
    // Optimization: Use filter_complex to cut and concat in a single pass.
    // Example:
    // ffmpeg -i input.mp4 -filter_complex
    // "[0:v]trim=start=10:end=20,setpts=PTS-STARTPTS[v0];
    //  [0:a]atrim=start=10:end=20,asetpts=PTS-STARTPTS[a0];
    //  [0:v]trim=start=30:end=40,setpts=PTS-STARTPTS[v1];
    //  [0:a]atrim=start=30:end=40,asetpts=PTS-STARTPTS[a1];
    //  [v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"
    // -map "[v]" -map "[a]" output.mp4

    let (filter_complex, _inputs) = build_filter_complex(segments);

    FfmpegCommand::new()
        .input(input_path.to_str().unwrap())
        .args(&[
            "-y",
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
        .for_each(|event| {
            if let FfmpegEvent::Progress(p) = event {
                on_progress(p.time);
            }
        });

    Ok(())
}

fn build_filter_complex(segments: &[Segment]) -> (String, String) {
    let mut filter_complex = String::new();
    let mut inputs = String::new();

    for (i, segment) in segments.iter().enumerate() {
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

    (filter_complex, inputs)
}

pub fn export_clips<F>(
    input_path: &Path,
    segments: &[ClipSegment],
    output_dir: &Path,
    on_progress: F,
) -> Result<()>
where
    F: Fn(String) + Send + 'static,
{
    if output_dir.exists() {
        if !output_dir.is_dir() {
            return Err(anyhow::anyhow!(
                "Output path exists and is not a directory: {:?}",
                output_dir
            ));
        }
    } else {
        std::fs::create_dir_all(output_dir).map_err(|e| {
            anyhow::anyhow!("Failed to create output directory {:?}: {}", output_dir, e)
        })?;
    }

    for (i, segment) in segments.iter().enumerate() {
        let output_filename = build_clip_output_filename(i, segment);
        let output_path = output_dir.join(output_filename);

        FfmpegCommand::new()
            .input(input_path.to_str().unwrap())
            .args(&[
                "-y",
                "-ss",
                &segment.start,
                "-to",
                &segment.end,
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
            ])
            .output(output_path.to_str().unwrap())
            .spawn()
            .map_err(|e| e.to_string())
            .map_err(anyhow::Error::msg)?
            .iter()
            .map_err(|e| e.to_string())
            .map_err(anyhow::Error::msg)?
            .for_each(|event| {
                if let FfmpegEvent::Progress(p) = event {
                    on_progress(p.time);
                }
            });
    }
    Ok(())
}

fn build_clip_output_filename(i: usize, segment: &ClipSegment) -> String {
    let suffix = segment
        .label
        .as_ref()
        .map(|l| l.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', ""))
        .unwrap_or_else(|| "".to_string());

    if suffix.is_empty() {
        format!("clip_{:03}.mp4", i + 1)
    } else {
        format!("clip_{:03}_{}.mp4", i + 1, suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_filter_complex() {
        let segments = vec![
            Segment {
                start: "00:00".to_string(),
                end: "00:10".to_string(),
            },
            Segment {
                start: "00:20".to_string(),
                end: "00:30".to_string(),
            },
        ];

        let (filter, inputs) = build_filter_complex(&segments);

        assert!(filter.contains("[0:v]trim=start=00:00:end=00:10,setpts=PTS-STARTPTS[v0];"));
        assert!(filter.contains("[0:a]atrim=start=00:00:end=00:10,asetpts=PTS-STARTPTS[a0];"));
        assert!(filter.contains("[0:v]trim=start=00:20:end=00:30,setpts=PTS-STARTPTS[v1];"));
        assert!(filter.contains("[0:a]atrim=start=00:20:end=00:30,asetpts=PTS-STARTPTS[a1];"));
        assert!(filter.contains("concat=n=2:v=1:a=1[v][a]"));
        assert_eq!(inputs, "[v0][a0][v1][a1]");
    }

    #[test]
    fn test_build_clip_output_filename() {
        let s1 = ClipSegment {
            start: "0".into(),
            end: "10".into(),
            label: None,
        };
        assert_eq!(build_clip_output_filename(0, &s1), "clip_001.mp4");

        let s2 = ClipSegment {
            start: "0".into(),
            end: "10".into(),
            label: Some("My Clip".into()),
        };
        assert_eq!(build_clip_output_filename(1, &s2), "clip_002_MyClip.mp4");

        let s3 = ClipSegment {
            start: "0".into(),
            end: "10".into(),
            label: Some("Clip/With\\BadChars!".into()),
        };
        assert_eq!(
            build_clip_output_filename(2, &s3),
            "clip_003_ClipWithBadChars.mp4"
        );
    }
}
