//! Morph cuts (shorts phase S4): a jump cut where the speaker barely moved is
//! hidden by interpolating a few frames between the two sides, like
//! Premiere's Morph Cut. Interpolation is RIFE 4.9 (Practical-RIFE, MIT),
//! downloaded on first use.
//!
//! Measured on the PODIUM recording (1280x720, CPU, 8 threads): 572 ms per
//! full frame, much less on the crop region used here. Frames 0.5-2 s apart
//! with a mean luma difference up to ~10 interpolate cleanly; hands moving a
//! lot start to ghost, hence the conservative [`MAX_DIFFERENCE`].

use crate::frames::{decode_frames, Frame, FrameRequest, PixelFormat};
use crate::model_download::PinnedFile;
use crate::reframe::CropRect;
use crate::run_control::RunControl;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

/// RIFE 4.9 as ONNX (`yuvraj108c/rife-onnx`, the export ComfyUI-Rife-
/// Tensorrt uses), checked against the repository's LFS metadata.
pub(crate) const RIFE: PinnedFile = PinnedFile {
    repo: "yuvraj108c/rife-onnx",
    revision: "64de7265b6a06637c2f2c6a92ecd1972326499fe",
    path: "rife49_ensemble_True_scale_1_sim.onnx",
    sha256: "76e4cef9ab42fa7dd4e8f6e4aba47462051e3faa969e4bca6479784fbab0ac6f",
    size: 21_458_882,
};

/// Sides more different than this (mean luma difference, 0-255) get a
/// punch-in instead of a morph.
pub(crate) const MAX_DIFFERENCE: f32 = 8.0;
/// In-between frames per morph.
pub(crate) const MORPH_FRAMES: usize = 6;

/// One frame of `path` at `time`, full width, RGB.
pub(crate) fn frame_at(
    path: &Path,
    time: f64,
    width: u32,
    run: Option<(u64, &RunControl)>,
) -> Result<Frame, String> {
    let mut frames = Vec::new();
    decode_frames(
        &FrameRequest {
            path,
            start: time.max(0.0),
            end: time.max(0.0) + 0.05,
            fps: 30.0,
            width,
            format: PixelFormat::Rgb24,
        },
        run,
        |frame| frames.push(frame),
    )?;
    frames
        .into_iter()
        .next()
        .ok_or_else(|| format!("No frame at {time:.2} s"))
}

/// The `rect` part of an RGB frame (clamped to it).
pub(crate) fn crop(frame: &Frame, rect: CropRect) -> Frame {
    let x0 = rect.x.min(frame.width.saturating_sub(1));
    let y0 = rect.y.min(frame.height.saturating_sub(1));
    let w = rect.width.min(frame.width - x0);
    let h = rect.height.min(frame.height - y0);
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in y0..y0 + h {
        let row = ((y * frame.width + x0) * 3) as usize;
        data.extend_from_slice(&frame.data[row..row + (w * 3) as usize]);
    }
    Frame {
        time: frame.time,
        width: w,
        height: h,
        data,
    }
}

/// Mean absolute luma difference of two same-sized RGB frames.
pub(crate) fn luma_difference(a: &Frame, b: &Frame) -> f32 {
    let luma =
        |p: &[u8; 3]| (u32::from(p[0]) * 77 + u32::from(p[1]) * 150 + u32::from(p[2]) * 29) >> 8;
    let (pa, pb) = (a.data.as_chunks::<3>().0, b.data.as_chunks::<3>().0);
    if pa.is_empty() || pa.len() != pb.len() {
        return f32::INFINITY;
    }
    let total: u64 = pa
        .iter()
        .zip(pb)
        .map(|(x, y)| u64::from(luma(x).abs_diff(luma(y))))
        .sum();
    total as f32 / pa.len() as f32
}

pub(crate) struct Morpher {
    session: Session,
}

impl Morpher {
    pub(crate) fn new(model: &Path) -> Result<Self, String> {
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(8);
        let session = Session::builder()
            .and_then(|builder| Ok(builder.with_intra_threads(threads)?))
            .and_then(|mut builder| builder.commit_from_file(model))
            .map_err(|e| format!("Failed to load the morph model: {e}"))?;
        Ok(Self { session })
    }

    /// `count` frames evenly spaced between `a` and `b` (same-sized RGB).
    pub(crate) fn between(
        &mut self,
        a: &Frame,
        b: &Frame,
        count: usize,
    ) -> Result<Vec<Frame>, String> {
        let (w, h) = (a.width as usize, a.height as usize);
        if (b.width, b.height) != (a.width, a.height) {
            return Err("Morph frames differ in size".to_string());
        }
        // RIFE works on multiples of 64; pad right and bottom.
        let (pad_w, pad_h) = (w.div_ceil(64) * 64, h.div_ceil(64) * 64);
        let plane = pad_w * pad_h;
        let tensor = |frame: &Frame| {
            let mut data = vec![0f32; 3 * plane];
            for y in 0..h {
                for x in 0..w {
                    let s = (y * w + x) * 3;
                    for c in 0..3 {
                        data[c * plane + y * pad_w + x] = f32::from(frame.data[s + c]) / 255.0;
                    }
                }
            }
            data
        };
        let (img0, img1) = (tensor(a), tensor(b));
        let mut frames = Vec::with_capacity(count);
        for k in 1..=count {
            let t = k as f32 / (count + 1) as f32;
            let input = |data: &Vec<f32>| {
                Tensor::from_array(([1usize, 3, pad_h, pad_w], data.clone()))
                    .map_err(|e| format!("Failed to prepare the morph input: {e}"))
            };
            let outputs = self
                .session
                .run(ort::inputs![
                    "img0" => input(&img0)?,
                    "img1" => input(&img1)?,
                    "timestep" => Tensor::from_array(([1usize], vec![t]))
                        .map_err(|e| format!("Failed to prepare the morph input: {e}"))?,
                ])
                .map_err(|e| format!("Morph interpolation failed: {e}"))?;
            let (_, out) = outputs["output"]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("Unexpected morph output: {e}"))?;
            let mut data = Vec::with_capacity(w * h * 3);
            for y in 0..h {
                for x in 0..w {
                    for c in 0..3 {
                        let value = out[c * plane + y * pad_w + x].clamp(0.0, 1.0);
                        data.push((value * 255.0).round() as u8);
                    }
                }
            }
            frames.push(Frame {
                time: a.time + (b.time - a.time) * f64::from(t),
                width: a.width,
                height: a.height,
                data,
            });
        }
        Ok(frames)
    }
}

/// Write an RGB frame as PNG.
pub(crate) fn write_png(path: &Path, frame: &Frame) -> Result<(), String> {
    let file = std::fs::File::create(path)
        .map_err(|e| format!("Failed to write '{}': {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&frame.data))
        .map_err(|e| format!("Failed to encode '{}': {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Frame {
        Frame {
            time: 0.0,
            width,
            height,
            data: rgb.repeat((width * height) as usize),
        }
    }

    #[test]
    fn luma_difference_measures_how_far_apart_frames_are() {
        let grey = solid(4, 4, [100, 100, 100]);
        assert_eq!(luma_difference(&grey, &grey), 0.0);
        let lighter = solid(4, 4, [110, 110, 110]);
        assert!((luma_difference(&grey, &lighter) - 10.0).abs() < 1.0);
        assert_eq!(
            luma_difference(&grey, &solid(2, 2, [0, 0, 0])),
            f32::INFINITY
        );
    }

    #[test]
    fn crops_are_taken_from_the_frame() {
        let mut frame = solid(4, 3, [0, 0, 0]);
        // Mark pixel (2, 1).
        let i = (4 + 2) * 3;
        frame.data[i] = 255;
        let part = crop(
            &frame,
            CropRect {
                x: 2,
                y: 1,
                width: 2,
                height: 2,
            },
        );
        assert_eq!((part.width, part.height), (2, 2));
        assert_eq!(part.data[0], 255);
        assert_eq!(part.data[3], 0);
    }

    /// Runs RIFE on a real jump cut. Needs the model at /tmp/rife/rife49.onnx
    /// (or `SHORTS_RIFE`) and `SHORTS_PROFILE_SOURCE`.
    #[test]
    #[ignore]
    fn morphs_a_real_jump_cut() {
        let Some(source) = std::env::var_os("SHORTS_PROFILE_SOURCE") else {
            eprintln!("SHORTS_PROFILE_SOURCE not set; skipping");
            return;
        };
        let model = std::env::var("SHORTS_RIFE").unwrap_or_else(|_| "/tmp/rife/rife49.onnx".into());
        let rect = CropRect {
            x: 880,
            y: 0,
            width: 270,
            height: 480,
        };
        let a = crop(
            &frame_at(Path::new(&source), 720.0, 1280, None).unwrap(),
            rect,
        );
        let b = crop(
            &frame_at(Path::new(&source), 720.5, 1280, None).unwrap(),
            rect,
        );
        let difference = luma_difference(&a, &b);
        let started = std::time::Instant::now();
        let frames = Morpher::new(Path::new(&model))
            .unwrap()
            .between(&a, &b, MORPH_FRAMES)
            .unwrap();
        println!(
            "difference {difference:.1}; {} frames of {}x{} in {:.0} ms",
            frames.len(),
            rect.width,
            rect.height,
            started.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(frames.len(), MORPH_FRAMES);
        // In-between frames are closer to both sides than the sides are to
        // each other.
        let middle = &frames[MORPH_FRAMES / 2];
        assert!(luma_difference(&a, middle) < difference);
        assert!(luma_difference(middle, &b) < difference);
    }
}
