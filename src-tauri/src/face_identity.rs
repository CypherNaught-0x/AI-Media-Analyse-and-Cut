//! Who is who across camera angles (shorts phase S2): a face embedding per
//! face, so the wide shot's seat and the close-up of the same person can be
//! listed (and ruled on) as one person.
//!
//! The embedding is SFace (OpenCV Zoo, Apache-2.0), the recognizer made to
//! go with YuNet: the face is aligned from YuNet's five landmarks onto the
//! 112x112 template OpenCV's `FaceRecognizerSF::alignCrop` uses, and the
//! 128-d feature is compared by cosine similarity. The model (39 MB) is
//! downloaded on first use.

use crate::faces::Face;
use crate::frames::Frame;
use crate::model_download::PinnedFile;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

/// `opencv/face_recognition_sface` (the OpenCV Zoo's 2021dec weights),
/// checked against the repository's LFS metadata.
pub(crate) const SFACE: PinnedFile = PinnedFile {
    repo: "opencv/face_recognition_sface",
    revision: "3d7082438a6e4551e840c9b2bb60b71e8da4b524",
    path: "face_recognition_sface_2021dec.onnx",
    sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
    size: 38_696_353,
};

const SIZE: usize = 112;
/// Where the eyes, nose tip and mouth corners go in the aligned crop.
const TEMPLATE: [(f32, f32); 5] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];

/// Faces at least this similar (cosine) are the same person.
pub(crate) const SAME_PERSON: f32 = 0.363;

/// A unit-length face embedding.
pub(crate) type Embedding = Vec<f32>;

pub(crate) struct FaceEmbedder {
    session: Session,
}

impl FaceEmbedder {
    pub(crate) fn new(model: &Path) -> Result<Self, String> {
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, 4);
        let session = Session::builder()
            .and_then(|builder| Ok(builder.with_intra_threads(threads)?))
            .and_then(|mut builder| builder.commit_from_file(model))
            .map_err(|e| format!("Failed to load the face recognition model: {e}"))?;
        Ok(Self { session })
    }

    /// The embedding of `face` in an RGB `frame` (same pixels).
    pub(crate) fn embed(&mut self, frame: &Frame, face: &Face) -> Result<Embedding, String> {
        let aligned = align(frame, face);
        let input = Tensor::from_array(([1usize, 3, SIZE, SIZE], aligned))
            .map_err(|e| format!("Failed to prepare the face recognition input: {e}"))?;
        let outputs = self
            .session
            .run(ort::inputs![input])
            .map_err(|e| format!("Face recognition failed: {e}"))?;
        let (_, feature) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("Unexpected face recognition output: {e}"))?;
        let norm = feature.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
        Ok(feature.iter().map(|v| v / norm).collect())
    }
}

/// Cosine similarity of two unit-length embeddings.
pub(crate) fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The similarity transform (a, b, tx, ty) taking the template onto the
/// landmarks, least squares: `x = a u - b v + tx`, `y = b u + a v + ty`.
fn template_to_face(landmarks: &[(f32, f32); 5]) -> (f32, f32, f32, f32) {
    let n = TEMPLATE.len() as f32;
    let mean = |points: &[(f32, f32)]| {
        let (x, y) = points
            .iter()
            .fold((0.0, 0.0), |(sx, sy), (x, y)| (sx + x, sy + y));
        (x / n, y / n)
    };
    let (mu, mv) = mean(&TEMPLATE);
    let (mx, my) = mean(landmarks);
    let (mut dot, mut cross, mut norm) = (0.0, 0.0, 0.0);
    for (&(u, v), &(x, y)) in TEMPLATE.iter().zip(landmarks) {
        let (u, v, x, y) = (u - mu, v - mv, x - mx, y - my);
        dot += u * x + v * y;
        cross += u * y - v * x;
        norm += u * u + v * v;
    }
    let (a, b) = (dot / norm.max(1e-6), cross / norm.max(1e-6));
    (a, b, mx - a * mu + b * mv, my - b * mu - a * mv)
}

/// The aligned 112x112 crop as NCHW RGB in 0-255 (OpenCV's blob for SFace).
fn align(frame: &Frame, face: &Face) -> Vec<f32> {
    let (a, b, tx, ty) = template_to_face(&face.landmarks);
    let (w, h) = (frame.width as usize, frame.height as usize);
    let plane = SIZE * SIZE;
    let mut blob = vec![0f32; 3 * plane];
    for v in 0..SIZE {
        for u in 0..SIZE {
            let (uf, vf) = (u as f32, v as f32);
            let x = a * uf - b * vf + tx;
            let y = b * uf + a * vf + ty;
            if x < 0.0 || y < 0.0 || x > (w - 1) as f32 || y > (h - 1) as f32 {
                continue;
            }
            let (x0, y0) = (x as usize, y as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            for channel in 0..3 {
                let pixel =
                    |px: usize, py: usize| f32::from(frame.data[(py * w + px) * 3 + channel]);
                let top = pixel(x0, y0) * (1.0 - fx) + pixel(x1, y0) * fx;
                let bottom = pixel(x0, y1) * (1.0 - fx) + pixel(x1, y1) * fx;
                blob[channel * plane + v * SIZE + u] = top * (1.0 - fy) + bottom * fy;
            }
        }
    }
    blob
}

/// Group faces into people. Each face has its camera setup and, if one
/// could be computed, an embedding. Faces of one setup are on screen
/// together, so they are never the same person; otherwise the most similar
/// groups merge first (average similarity) while at least `threshold`.
/// Returns a group number per face, numbered in order of first face.
pub(crate) fn group_people(faces: &[(usize, Option<&[f32]>)], threshold: f32) -> Vec<usize> {
    let mut groups: Vec<Vec<usize>> = (0..faces.len()).map(|i| vec![i]).collect();
    let pair = |a: &[usize], b: &[usize]| -> Option<f32> {
        let mut total = 0.0;
        for &i in a {
            for &j in b {
                if faces[i].0 == faces[j].0 {
                    return None;
                }
                total += similarity(faces[i].1?, faces[j].1?);
            }
        }
        Some(total / (a.len() * b.len()) as f32)
    };
    loop {
        let mut best: Option<(f32, usize, usize)> = None;
        for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                if let Some(score) = pair(&groups[i], &groups[j]) {
                    if score >= threshold && best.is_none_or(|(s, _, _)| score > s) {
                        best = Some((score, i, j));
                    }
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        let merged = groups.remove(j);
        groups[i].extend(merged);
    }
    groups.sort_by_key(|group| group.iter().copied().min());
    let mut person = vec![0; faces.len()];
    for (number, group) in groups.iter().enumerate() {
        for &face in group {
            person[face] = number;
        }
    }
    person
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_maps_onto_a_scaled_shifted_copy() {
        let landmarks = TEMPLATE.map(|(u, v)| (2.0 * u + 100.0, 2.0 * v + 50.0));
        let (a, b, tx, ty) = template_to_face(&landmarks);
        assert!((a - 2.0).abs() < 1e-4 && b.abs() < 1e-4, "{a} {b}");
        assert!(
            (tx - 100.0).abs() < 1e-3 && (ty - 50.0).abs() < 1e-3,
            "{tx} {ty}"
        );
    }

    #[test]
    fn a_rotated_face_is_turned_upright() {
        // The template turned 90 degrees: x = -v, y = u.
        let landmarks = TEMPLATE.map(|(u, v)| (300.0 - v, u + 20.0));
        let (a, b, _, _) = template_to_face(&landmarks);
        assert!(a.abs() < 1e-4 && (b - 1.0).abs() < 1e-4, "{a} {b}");
    }

    #[test]
    fn people_group_across_setups_but_never_within_one() {
        let unit = |x: f32, y: f32| {
            let n = (x * x + y * y).sqrt();
            vec![x / n, y / n]
        };
        let (anna, bob) = (unit(1.0, 0.1), unit(0.1, 1.0));
        let anna_close = unit(1.0, 0.2);
        // Like Anna, but less than her close-up, which is in its setup.
        let lookalike = unit(1.0, -0.6);
        let faces = [
            (0, Some(anna.as_slice())),
            (0, Some(bob.as_slice())),
            (1, Some(anna_close.as_slice())),
            (1, Some(lookalike.as_slice())),
            (2, None),
        ];
        assert_eq!(group_people(&faces, SAME_PERSON), [0, 1, 0, 2, 3]);
    }
}
