//! Which H.264 encoder exports use, and with what settings.
//!
//! Hardware encoders are much faster than libx264 on long exports. Being
//! listed by `ffmpeg -encoders` doesn't mean one works (NVENC is compiled into
//! many builds without an NVIDIA GPU present), so each candidate is verified
//! by encoding a single frame before it is chosen.

use ffmpeg_sidecar::paths::ffmpeg_path;
use log::info;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Encoder {
    /// Apple VideoToolbox (macOS).
    VideoToolbox,
    /// NVIDIA NVENC.
    Nvenc,
    /// Intel Quick Sync.
    Qsv,
    /// AMD AMF (Windows).
    Amf,
    /// Software fallback, always available in the builds the app uses.
    X264,
}

impl H264Encoder {
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::VideoToolbox => "h264_videotoolbox",
            Self::Nvenc => "h264_nvenc",
            Self::Qsv => "h264_qsv",
            Self::Amf => "h264_amf",
            Self::X264 => "libx264",
        }
    }

    /// Hardware encoders worth trying on this platform, best first.
    fn hardware_candidates() -> &'static [H264Encoder] {
        if cfg!(target_os = "macos") {
            &[Self::VideoToolbox]
        } else if cfg!(target_os = "windows") {
            &[Self::Nvenc, Self::Qsv, Self::Amf]
        } else {
            &[Self::Nvenc, Self::Qsv]
        }
    }
}

/// Speed/quality trade-off for an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ExportQuality {
    /// Visually lossless; for final masters.
    High,
    /// Good quality at a sensible size; the default for clips and cuts.
    #[default]
    Balanced,
    /// Fast and small; previews and drafts.
    Draft,
}

/// Video and audio encoding arguments (everything after the filters/maps):
/// H.264 in yuv420p for broad playback, AAC audio, and the moov atom up front
/// so files stream and seek immediately.
pub fn encode_args(encoder: H264Encoder, quality: ExportQuality) -> Vec<String> {
    use ExportQuality::*;
    let video: Vec<&str> = match encoder {
        H264Encoder::X264 => {
            let (preset, crf) = match quality {
                High => ("slow", "17"),
                Balanced => ("veryfast", "20"),
                Draft => ("ultrafast", "26"),
            };
            vec!["-preset", preset, "-crf", crf]
        }
        // Constant-quality mode (Apple Silicon); higher is better.
        H264Encoder::VideoToolbox => {
            let q = match quality {
                High => "75",
                Balanced => "62",
                Draft => "45",
            };
            vec!["-q:v", q, "-allow_sw", "1"]
        }
        H264Encoder::Nvenc => {
            let (preset, cq) = match quality {
                High => ("p6", "18"),
                Balanced => ("p4", "22"),
                Draft => ("p1", "28"),
            };
            vec!["-preset", preset, "-rc", "vbr", "-cq", cq, "-b:v", "0"]
        }
        H264Encoder::Qsv => {
            let q = match quality {
                High => "18",
                Balanced => "22",
                Draft => "28",
            };
            vec!["-global_quality", q]
        }
        H264Encoder::Amf => {
            let (mode, qp) = match quality {
                High => ("quality", "18"),
                Balanced => ("balanced", "22"),
                Draft => ("speed", "28"),
            };
            vec!["-quality", mode, "-rc", "cqp", "-qp_i", qp, "-qp_p", qp]
        }
    };

    let mut args = vec!["-c:v", encoder.ffmpeg_name()];
    args.extend(video);
    args.extend([
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-movflags",
        "+faststart",
    ]);
    args.into_iter().map(String::from).collect()
}

/// The encoder exports use: the first working hardware encoder, else libx264.
/// Probed once per app session.
pub fn preferred_h264_encoder() -> H264Encoder {
    static SELECTED: OnceLock<H264Encoder> = OnceLock::new();
    *SELECTED.get_or_init(|| {
        let selected = H264Encoder::hardware_candidates()
            .iter()
            .copied()
            .find(|encoder| encoder_works(*encoder))
            .unwrap_or(H264Encoder::X264);
        info!("Using {} for video exports", selected.ffmpeg_name());
        selected
    })
}

/// Encode one small frame with `encoder`; true if ffmpeg succeeds.
fn encoder_works(encoder: H264Encoder) -> bool {
    Command::new(ffmpeg_path())
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "color=c=black:s=256x256:d=0.04"])
        .args(["-frames:v", "1", "-c:v", encoder.ffmpeg_name()])
        .args(["-pix_fmt", "yuv420p", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_encoding_ends_with_playable_container_settings() {
        for encoder in [
            H264Encoder::X264,
            H264Encoder::VideoToolbox,
            H264Encoder::Nvenc,
            H264Encoder::Qsv,
            H264Encoder::Amf,
        ] {
            for quality in [
                ExportQuality::High,
                ExportQuality::Balanced,
                ExportQuality::Draft,
            ] {
                let args = encode_args(encoder, quality);
                assert_eq!(args[..2], ["-c:v", encoder.ffmpeg_name()]);
                assert!(args.windows(2).any(|w| w == ["-pix_fmt", "yuv420p"]));
                assert!(args.windows(2).any(|w| w == ["-movflags", "+faststart"]));
                assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
            }
        }
    }

    #[test]
    fn libx264_trades_speed_for_quality_by_preset() {
        let crf = |quality| {
            let args = encode_args(H264Encoder::X264, quality);
            let i = args.iter().position(|a| a == "-crf").unwrap();
            args[i + 1].parse::<u32>().unwrap()
        };
        assert!(crf(ExportQuality::High) < crf(ExportQuality::Balanced));
        assert!(crf(ExportQuality::Balanced) < crf(ExportQuality::Draft));
    }

    /// Needs ffmpeg on PATH (as the other ffmpeg tests do). libx264 is always
    /// in the builds the app uses, and the selection must be something that
    /// actually encodes on this machine.
    #[test]
    fn the_selected_encoder_works_here() {
        assert!(encoder_works(H264Encoder::X264));
        assert!(encoder_works(preferred_h264_encoder()));
    }
}
