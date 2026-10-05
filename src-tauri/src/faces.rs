//! Face detection (shorts phase S2) with YuNet, a small (230 KB) detector
//! from the OpenCV Zoo (MIT). The model is embedded; it is the 2023mar
//! weights re-exported with dynamic input size (`2026may`), pinned at
//! opencv_zoo 26cc381e, SHA-256 ebafce4e…22f0f0.
//!
//! Decoding follows OpenCV's `FaceDetectorYN`: BGR input in 0-255 padded to
//! a multiple of 32, three anchor-free heads (strides 8, 16, 32).

use crate::frames::Frame;
use ort::session::Session;
use ort::value::Tensor;

static MODEL: &[u8] = include_bytes!("../models/face_detection_yunet_2026may.onnx");

const STRIDES: [u32; 3] = [8, 16, 32];
/// Minimum `sqrt(class * objectness)`. OpenCV's demo uses 0.9; small faces
/// in panel wide shots score lower.
const SCORE_THRESHOLD: f32 = 0.7;
/// Boxes overlapping a better one by more than this IoU are dropped.
const NMS_IOU: f32 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Face {
    /// Box in frame pixels.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub score: f32,
    /// The subject's right eye, left eye, nose tip, right and left mouth
    /// corner, in frame pixels.
    pub landmarks: [(f32, f32); 5],
}

impl Face {
    pub(crate) fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub(crate) fn iou(&self, other: &Face) -> f32 {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        let overlap = (right - left).max(0.0) * (bottom - top).max(0.0);
        let union = self.width * self.height + other.width * other.height - overlap;
        if union <= 0.0 {
            0.0
        } else {
            overlap / union
        }
    }
}

pub(crate) struct FaceDetector {
    session: Session,
}

impl FaceDetector {
    pub(crate) fn new() -> Result<Self, String> {
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, 4);
        let session = Session::builder()
            .and_then(|builder| Ok(builder.with_intra_threads(threads)?))
            .and_then(|mut builder| builder.commit_from_memory(MODEL))
            .map_err(|e| format!("Failed to load the face detection model: {e}"))?;
        Ok(Self { session })
    }

    /// Faces in an RGB frame, best first.
    pub(crate) fn detect(&mut self, frame: &Frame) -> Result<Vec<Face>, String> {
        let (width, height) = (frame.width as usize, frame.height as usize);
        if frame.data.len() != width * height * 3 {
            return Err("Face detection needs RGB frames".to_string());
        }
        let pad = |value: usize| value.div_ceil(32) * 32;
        let (pad_w, pad_h) = (pad(width), pad(height));

        // NCHW, BGR, zero padding right and bottom.
        let plane = pad_w * pad_h;
        let mut input = vec![0f32; 3 * plane];
        for y in 0..height {
            for x in 0..width {
                let source = (y * width + x) * 3;
                let target = y * pad_w + x;
                input[target] = f32::from(frame.data[source + 2]);
                input[plane + target] = f32::from(frame.data[source + 1]);
                input[2 * plane + target] = f32::from(frame.data[source]);
            }
        }
        let input = Tensor::from_array(([1usize, 3, pad_h, pad_w], input))
            .map_err(|e| format!("Failed to prepare the face detection input: {e}"))?;
        let outputs = self
            .session
            .run(ort::inputs!["input" => input])
            .map_err(|e| format!("Face detection failed: {e}"))?;

        let mut faces = Vec::new();
        for stride in STRIDES {
            let head = |name: &str| -> Result<&[f32], String> {
                outputs[format!("{name}_{stride}").as_str()]
                    .try_extract_tensor::<f32>()
                    .map(|(_, data)| data)
                    .map_err(|e| format!("Unexpected face detection output: {e}"))
            };
            decode_level(
                &Level {
                    stride,
                    cols: pad_w / stride as usize,
                    rows: pad_h / stride as usize,
                    cls: head("cls")?,
                    obj: head("obj")?,
                    bbox: head("bbox")?,
                    kps: head("kps")?,
                },
                SCORE_THRESHOLD,
                &mut faces,
            );
        }
        Ok(non_max_suppression(faces, NMS_IOU))
    }
}

/// One detection head's raw outputs.
struct Level<'a> {
    stride: u32,
    cols: usize,
    rows: usize,
    cls: &'a [f32],
    obj: &'a [f32],
    bbox: &'a [f32],
    kps: &'a [f32],
}

fn decode_level(level: &Level<'_>, threshold: f32, faces: &mut Vec<Face>) {
    let stride = level.stride as f32;
    let anchors = (level.rows * level.cols)
        .min(level.cls.len())
        .min(level.obj.len())
        .min(level.bbox.len() / 4)
        .min(level.kps.len() / 10);
    for idx in 0..anchors {
        let score = (level.cls[idx].clamp(0.0, 1.0) * level.obj[idx].clamp(0.0, 1.0)).sqrt();
        if score < threshold {
            continue;
        }
        let (row, col) = ((idx / level.cols) as f32, (idx % level.cols) as f32);
        let bbox = &level.bbox[idx * 4..idx * 4 + 4];
        let center_x = (col + bbox[0]) * stride;
        let center_y = (row + bbox[1]) * stride;
        let width = bbox[2].exp() * stride;
        let height = bbox[3].exp() * stride;
        let kps = &level.kps[idx * 10..idx * 10 + 10];
        let landmark = |n: usize| ((kps[2 * n] + col) * stride, (kps[2 * n + 1] + row) * stride);
        faces.push(Face {
            x: center_x - width / 2.0,
            y: center_y - height / 2.0,
            width,
            height,
            score,
            landmarks: [0, 1, 2, 3, 4].map(landmark),
        });
    }
}

fn non_max_suppression(mut faces: Vec<Face>, max_iou: f32) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::with_capacity(faces.len());
    for face in faces {
        if kept.iter().all(|better| better.iou(&face) <= max_iou) {
            kept.push(face);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32, size: f32, score: f32) -> Face {
        Face {
            x,
            y,
            width: size,
            height: size,
            score,
            landmarks: [(0.0, 0.0); 5],
        }
    }

    #[test]
    fn decodes_anchor_offsets_like_opencv() {
        // 4x3 grid at stride 8; one confident anchor at row 1, column 2.
        let (cols, rows) = (4, 3);
        let mut cls = vec![0.0; cols * rows];
        let mut obj = vec![0.0; cols * rows];
        let mut bbox = vec![0.0; cols * rows * 4];
        let mut kps = vec![0.0; cols * rows * 10];
        let idx = cols + 2;
        cls[idx] = 1.2; // clamped to 1
        obj[idx] = 0.81;
        bbox[idx * 4..idx * 4 + 4].copy_from_slice(&[0.5, 0.5, 2f32.ln(), 4f32.ln()]);
        kps[idx * 10] = 0.25;
        kps[idx * 10 + 1] = -0.5;
        // Below the threshold: ignored.
        cls[0] = 0.5;
        obj[0] = 0.5;

        let mut faces = Vec::new();
        decode_level(
            &Level {
                stride: 8,
                cols,
                rows,
                cls: &cls,
                obj: &obj,
                bbox: &bbox,
                kps: &kps,
            },
            0.7,
            &mut faces,
        );
        assert_eq!(faces.len(), 1);
        let found = faces[0];
        assert!((found.score - 0.9).abs() < 1e-6);
        // Centre (2.5, 1.5) cells = (20, 12) px; 16x32 px.
        assert_eq!(
            (found.x, found.y, found.width, found.height),
            (12.0, -4.0, 16.0, 32.0)
        );
        assert_eq!(found.landmarks[0], (18.0, 4.0));
        assert_eq!(found.center(), (20.0, 12.0));
    }

    #[test]
    fn overlapping_detections_keep_the_best() {
        let kept = non_max_suppression(
            vec![
                face(0.0, 0.0, 10.0, 0.8),
                face(1.0, 1.0, 10.0, 0.95),
                face(50.0, 0.0, 10.0, 0.75),
            ],
            0.3,
        );
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.95);
        assert_eq!(kept[1].x, 50.0);
    }

    #[test]
    fn the_embedded_model_runs_and_finds_nothing_in_a_flat_frame() {
        let mut detector = FaceDetector::new().unwrap();
        let frame = Frame {
            time: 0.0,
            width: 100,
            height: 70,
            data: vec![128; 100 * 70 * 3],
        };
        assert!(detector.detect(&frame).unwrap().is_empty());
    }
}

#[cfg(test)]
mod profiling {
    use super::*;
    use crate::frames::{decode_frames, FrameRequest, PixelFormat};
    use std::time::Instant;

    /// Detects faces in 60 s of a real source at 2 fps and reports speed and
    /// face counts. With `SHORTS_PROFILE_KEEP` set, writes every 10th frame
    /// with boxes and landmarks drawn as PPM.
    /// `cargo test --release --lib yunet_throughput -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn yunet_throughput() {
        let Some(source) = std::env::var_os("SHORTS_PROFILE_SOURCE") else {
            eprintln!("SHORTS_PROFILE_SOURCE not set; skipping");
            return;
        };
        let keep = std::env::var_os("SHORTS_PROFILE_KEEP").map(std::path::PathBuf::from);
        let start = std::env::var("SHORTS_PROFILE_START")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(600.0);
        let width = std::env::var("SHORTS_PROFILE_WIDTH")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(640);
        let mut frames = Vec::new();
        let started = Instant::now();
        decode_frames(
            &FrameRequest {
                path: std::path::Path::new(&source),
                start,
                end: start + 60.0,
                fps: 2.0,
                width,
                format: PixelFormat::Rgb24,
            },
            None,
            |frame| frames.push(frame),
        )
        .unwrap();
        let decoding = started.elapsed().as_secs_f64();

        let mut detector = FaceDetector::new().unwrap();
        let started = Instant::now();
        let mut counts = std::collections::BTreeMap::<usize, usize>::new();
        let mut scores = Vec::new();
        for (i, frame) in frames.iter().enumerate() {
            let faces = detector.detect(frame).unwrap();
            *counts.entry(faces.len()).or_default() += 1;
            scores.extend(faces.iter().map(|face| face.score));
            if let (Some(dir), 0) = (&keep, i % 10) {
                write_annotated(&dir.join(format!("faces_{i:03}.ppm")), frame, &faces);
            }
        }
        let detecting = started.elapsed().as_secs_f64();
        let (w, h) = (frames[0].width, frames[0].height);
        println!(
            "{} frames {w}x{h}: decode {decoding:.2} s, detect {detecting:.2} s ({:.1} ms/frame)",
            frames.len(),
            detecting * 1000.0 / frames.len() as f64
        );
        println!("faces per frame -> frames: {counts:?}");
        scores.sort_by(f32::total_cmp);
        if !scores.is_empty() {
            println!(
                "scores: min {:.2}, median {:.2}",
                scores[0],
                scores[scores.len() / 2]
            );
        }
    }

    fn write_annotated(path: &std::path::Path, frame: &Frame, faces: &[Face]) {
        let (w, h) = (frame.width as i64, frame.height as i64);
        let mut data = frame.data.clone();
        let mut dot = |x: f32, y: f32, colour: [u8; 3]| {
            let (x, y) = (x as i64, y as i64);
            if (0..w).contains(&x) && (0..h).contains(&y) {
                let i = ((y * w + x) * 3) as usize;
                data[i..i + 3].copy_from_slice(&colour);
            }
        };
        for face in faces {
            for t in 0..=100 {
                let t = t as f32 / 100.0;
                for (x, y) in [
                    (face.x + face.width * t, face.y),
                    (face.x + face.width * t, face.y + face.height),
                    (face.x, face.y + face.height * t),
                    (face.x + face.width, face.y + face.height * t),
                ] {
                    dot(x, y, [0, 255, 0]);
                }
            }
            for (x, y) in face.landmarks {
                for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                    dot(x + dx, y + dy, [255, 0, 0]);
                }
            }
        }
        let mut file = format!("P6\n{w} {h}\n255\n").into_bytes();
        file.extend_from_slice(&data);
        std::fs::write(path, file).unwrap();
    }
}
