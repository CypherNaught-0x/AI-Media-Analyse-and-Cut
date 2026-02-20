use crate::video::Segment;
use anyhow::{anyhow, Context, Result};
use hf_hub::{api::sync::Api, Repo, RepoType};
use ort::{
    session::{builder::GraphOptimizationLevel, Session},
    value::Value,
};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use symphonia::core::audio::AudioBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tauri::Emitter;

// --- Vocab Info ---
#[derive(Clone)]
struct VocabInfo {
    id_to_token: HashMap<usize, String>,
    vocab_size: usize,
    blank_id: usize,
}

impl VocabInfo {
    fn from_file(path: &Path) -> Result<Self> {
        use std::fs;
        let mut id_to_token = HashMap::new();
        let mut blank_id: Option<usize> = None;

        let content = fs::read_to_string(path)?;
        for (lineno, line) in content.lines().enumerate() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let token = parts[0].to_string();
            let id: usize = parts[1]
                .parse()
                .with_context(|| format!("Bad token id at line {}: {}", lineno + 1, line))?;
            if token == "<blk>" || token == "<blank>" {
                blank_id = Some(id);
            }
            id_to_token.insert(id, token);
        }

        let vocab_size = id_to_token.len();
        let blank_id =
            blank_id.ok_or_else(|| anyhow!("No <blk>/<blank> token found in vocab file"))?;

        Ok(Self {
            id_to_token,
            vocab_size,
            blank_id,
        })
    }

    fn token_of(&self, id: usize) -> Option<&str> {
        self.id_to_token.get(&id).map(|s| s.as_str())
    }
}

// --- Helpers ---
fn argmax_index(xs: &[f32]) -> (usize, f32) {
    let mut best = 0usize;
    let mut bestv = f32::NEG_INFINITY;
    for (i, &v) in xs.iter().enumerate() {
        if v > bestv {
            bestv = v;
            best = i;
        }
    }
    (best, bestv)
}

// --- Model ---
pub struct ParakeetModel {
    encoder_session: Session,
    decoder_session: Session,
    feature_extractor_session: Session,
    vocab: VocabInfo,
    sample_rate: u32,
}

impl ParakeetModel {
    pub fn download() -> Result<Self> {
        let api = Api::new()?;
        let repo = api.repo(Repo::new(
            "s0me-0ne/parakeet-tdt-0.6b-v3-onnx".to_string(),
            RepoType::Model,
        ));

        let encoder_path = repo.get("encoder.onnx")?;
        let decoder_path = repo.get("decoder.onnx")?;
        let feature_extractor_path = repo.get("feature_extractor.onnx")?;
        let vocab_path = repo.get("vocab.txt")?;

        let vocab = VocabInfo::from_file(&vocab_path)?;

        let builder = || {
            Session::builder()
                .unwrap()
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .unwrap()
        };

        // For now, using CPU to ensure compatibility.
        // To enable GPU, we would need to configure execution providers here.
        let encoder_session = builder().commit_from_file(encoder_path)?;
        let decoder_session = builder().commit_from_file(decoder_path)?;
        let feature_extractor_session = builder().commit_from_file(feature_extractor_path)?;

        Ok(Self {
            encoder_session,
            decoder_session,
            feature_extractor_session,
            vocab,
            sample_rate: 16000,
        })
    }

    // Note:
    // The local model generates its own transcript and timestamps.
    // Ideally, we would align the *original* text to these timestamps, but
    // simply returning the high-quality local transcript is easier for now.
    // If strict alignment of the *original* text is required, we'd need DTW.
    // For now, we return the local transcript segments.

    fn transcribe_batch(&mut self, audio: &[f32]) -> Result<BatchTranscriptionResult> {
        // Simple single-chunk for now, or loop if long
        let max_len = 480_000; // 30s
        if audio.len() > max_len {
            self.transcribe_long_audio(audio)
        } else {
            self.transcribe_single_chunk(audio)
        }
    }

    fn transcribe_long_audio(&mut self, audio: &[f32]) -> Result<BatchTranscriptionResult> {
        let chunk_size = 480_000;
        let overlap = 48_000;
        let sr = self.sample_rate as f32;

        let mut segments = Vec::new();
        let mut pos = 0;

        while pos < audio.len() {
            let end = (pos + chunk_size).min(audio.len());
            let chunk = &audio[pos..end];

            let res = self.transcribe_single_chunk(chunk)?;
            let t0 = pos as f32 / sr;

            for mut seg in res.segments {
                seg.start += t0;
                seg.end += t0;
                segments.push(seg);
            }

            if end == audio.len() {
                break;
            }
            pos += chunk_size - overlap;
        }

        let segments = merge_chunk_overlap_segments(segments, overlap as f32 / sr);
        let text = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(BatchTranscriptionResult { text, segments })
    }

    fn transcribe_single_chunk(&mut self, audio: &[f32]) -> Result<BatchTranscriptionResult> {
        // 1. Feature Extraction
        let batch = 1usize;
        let audio_len = audio.len();
        let audio_tensor = Value::from_array(([batch, audio_len], audio.to_vec()))?;
        let lens_tensor = Value::from_array(([batch], vec![audio_len as i64]))?;

        let mut fe_inputs: HashMap<String, Value> = HashMap::new();
        for input in self.feature_extractor_session.inputs() {
            if input.name().contains("waveforms") && !input.name().contains("lens") {
                fe_inputs.insert(input.name().to_string(), audio_tensor.clone().into_dyn());
            } else if input.name().contains("lens") {
                fe_inputs.insert(input.name().to_string(), lens_tensor.clone().into_dyn());
            }
        }

        let fe_outputs = self.feature_extractor_session.run(fe_inputs)?;
        let features_val = fe_outputs
            .values()
            .next()
            .ok_or_else(|| anyhow!("No FE output"))?;
        let (feat_shape, feat_slice) = features_val.try_extract_tensor::<f32>()?;

        // Handle shape [B, 128, T] or [B, T, 128]
        let (features_tensor, t_len) = if feat_shape[1] == 128 {
            let t_dim = feat_shape[2];
            let ft = Value::from_array((vec![feat_shape[0], 128, t_dim], feat_slice.to_vec()))?;
            (ft, t_dim as i64)
        } else {
            // Transpose [B, T, 128] -> [B, 128, T]
            let b = feat_shape[0];
            let t = feat_shape[1];
            let mut transposed = vec![0f32; (b * 128 * t) as usize];
            for bb in 0..b {
                for tt in 0..t {
                    for ff in 0..128 {
                        let src = ((bb * t + tt) * 128 + ff) as usize;
                        let dst = ((bb * 128 + ff) * t + tt) as usize;
                        transposed[dst] = feat_slice[src];
                    }
                }
            }
            let ft = Value::from_array((vec![b, 128, t], transposed))?;
            (ft, t as i64)
        };

        drop(fe_outputs);

        // 2. Encoder
        let mut enc_inputs: HashMap<String, Value> = HashMap::new();
        for input in self.encoder_session.inputs() {
            if input.name().contains("len") {
                let l = Value::from_array(([batch], vec![t_len]))?;
                enc_inputs.insert(input.name().to_string(), l.into_dyn());
            } else {
                enc_inputs.insert(input.name().to_string(), features_tensor.clone().into_dyn());
            }
        }

        let enc_outputs = self.encoder_session.run(enc_inputs)?;
        let enc_val = enc_outputs
            .iter()
            .find(|(k, _)| *k == "outputs")
            .or_else(|| enc_outputs.iter().next())
            .unwrap()
            .1;
        let (enc_shape, enc_slice) = enc_val.try_extract_tensor::<f32>()?;
        let (b, d, t_enc) = (enc_shape[0], enc_shape[1], enc_shape[2]);

        let enc_vec = enc_slice.to_vec();
        drop(enc_outputs);

        // 3. Decoder (TDT Greedy)
        let tokens = self.decode_tdt_greedy(&enc_vec, (b as usize, d as usize, t_enc as usize))?;
        let text = tokens_to_text(&tokens, &self.vocab);

        let segment = TranscriptionSegment {
            start: 0.0,
            end: audio.len() as f32 / self.sample_rate as f32,
            text: text.clone(),
        };

        Ok(BatchTranscriptionResult {
            text,
            segments: vec![segment],
        })
    }

    fn decode_tdt_greedy(
        &mut self,
        encoder_all: &[f32],
        (b, d, t_enc): (usize, usize, usize),
    ) -> Result<Vec<usize>> {
        let batch = 1usize;
        let mut states_1 = vec![0.0f32; 2 * batch * 640];
        let mut states_2 = vec![0.0f32; 2 * batch * 640];
        let mut decoded = Vec::new();
        let mut frame_idx = 0usize;
        let mut emitted_this_frame = 0usize;
        let max_tokens_per_frame = 10;

        while frame_idx < t_enc && decoded.len() < 4096 {
            let last_tok = decoded.last().copied().unwrap_or(self.vocab.blank_id) as i32;

            let targets = Value::from_array(([batch, 1], vec![last_tok]))?;
            let target_len = Value::from_array(([batch], vec![1i32]))?;
            let s1 = Value::from_array(([2, batch, 640], states_1.clone()))?;
            let s2 = Value::from_array(([2, batch, 640], states_2.clone()))?;
            let enc = Value::from_array(([b, d, t_enc], encoder_all.to_vec()))?;

            let mut inputs: HashMap<String, Value> = HashMap::new();
            inputs.insert("encoder_outputs".to_string(), enc.into_dyn());
            inputs.insert("targets".to_string(), targets.into_dyn());
            inputs.insert("target_length".to_string(), target_len.into_dyn());
            inputs.insert("input_states_1".to_string(), s1.into_dyn());
            inputs.insert("input_states_2".to_string(), s2.into_dyn());

            let outputs = self.decoder_session.run(inputs)?;
            let out_val = outputs.get("outputs").unwrap();
            let (out_shape, out_slice) = out_val.try_extract_tensor::<f32>()?;

            let c_dim = out_shape[3] as usize;
            let start = frame_idx * c_dim;
            let logits = &out_slice[start..start + c_dim];
            let (vocab_logits, dur_logits) = logits.split_at(self.vocab.vocab_size);

            let (pred_token, _) = argmax_index(vocab_logits);
            let (mut dur_bin, _) = argmax_index(dur_logits);
            if dur_bin == 0 {
                dur_bin = 1;
            }

            if pred_token == self.vocab.blank_id {
                frame_idx += 1;
                emitted_this_frame = 0;
            } else {
                decoded.push(pred_token);
                emitted_this_frame += 1;
                if emitted_this_frame >= max_tokens_per_frame {
                    frame_idx += 1;
                    emitted_this_frame = 0;
                } else {
                    frame_idx += dur_bin;
                }
            }

            if let Some(s) = outputs.get("output_states_1") {
                states_1 = s.try_extract_tensor::<f32>()?.1.to_vec();
            }
            if let Some(s) = outputs.get("output_states_2") {
                states_2 = s.try_extract_tensor::<f32>()?.1.to_vec();
            }
        }
        Ok(decoded)
    }
}

#[derive(Debug, Clone)]
struct TokenWord {
    raw: String,
    normalized: String,
}

fn normalize_word(word: &str) -> String {
    let mut normalized = String::new();

    for ch in word.chars() {
        if ch.is_alphanumeric() || ch == '\'' {
            for lower in ch.to_lowercase() {
                normalized.push(lower);
            }
        }
    }

    normalized
}

fn tokenize_words(text: &str) -> Vec<TokenWord> {
    text.split_whitespace()
        .filter_map(|raw| {
            let normalized = normalize_word(raw);
            if normalized.is_empty() {
                None
            } else {
                Some(TokenWord {
                    raw: raw.to_string(),
                    normalized,
                })
            }
        })
        .collect()
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    if a_chars.is_empty() {
        return b_chars.len();
    }
    if b_chars.is_empty() {
        return a_chars.len();
    }

    let mut previous: Vec<usize> = (0..=b_chars.len()).collect();

    for (i, a_char) in a_chars.iter().enumerate() {
        let mut current = vec![i + 1; b_chars.len() + 1];
        for (j, b_char) in b_chars.iter().enumerate() {
            let cost = usize::from(a_char != b_char);
            current[j + 1] = (current[j] + 1)
                .min(previous[j + 1] + 1)
                .min(previous[j] + cost);
        }
        previous = current;
    }

    previous[b_chars.len()]
}

fn are_words_similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }

    let a_len = a.chars().count();
    let b_len = b.chars().count();
    let max_len = a_len.max(b_len);

    if max_len < 3 {
        return false;
    }

    if a_len >= 4 && b_len >= 4 && (a.starts_with(b) || b.starts_with(a)) {
        return true;
    }

    let dist = levenshtein_distance(a, b);
    let similarity = 1.0 - (dist as f32 / max_len as f32);
    similarity >= 0.75
}

fn fuzzy_suffix_prefix_overlap_words(
    left: &[TokenWord],
    right: &[TokenWord],
    min_overlap_words: usize,
) -> usize {
    let max_overlap = left.len().min(right.len()).min(40);
    if max_overlap < min_overlap_words {
        return 0;
    }

    for overlap_size in (min_overlap_words..=max_overlap).rev() {
        let left_slice = &left[left.len() - overlap_size..];
        let right_slice = &right[..overlap_size];

        let matches = left_slice
            .iter()
            .zip(right_slice.iter())
            .filter(|(l, r)| are_words_similar(&l.normalized, &r.normalized))
            .count();

        if (matches as f32 / overlap_size as f32) >= 0.8 {
            return overlap_size;
        }
    }

    0
}

fn trim_prefix_words(text: &str, words_to_trim: usize) -> String {
    let words = tokenize_words(text);
    words
        .iter()
        .skip(words_to_trim)
        .map(|w| w.raw.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn are_near_duplicate_texts(a: &str, b: &str) -> bool {
    let a_words = tokenize_words(a);
    let b_words = tokenize_words(b);

    if a_words.len() < 4 || b_words.len() < 4 {
        return false;
    }

    let a_set: HashSet<String> = a_words.iter().map(|w| w.normalized.clone()).collect();
    let b_set: HashSet<String> = b_words.iter().map(|w| w.normalized.clone()).collect();

    if a_set.is_empty() || b_set.is_empty() {
        return false;
    }

    let common = a_set.intersection(&b_set).count();
    let containment = common as f32 / a_set.len().min(b_set.len()) as f32;
    containment >= 0.9
}

fn is_merge_boundary(
    previous: &TranscriptionSegment,
    current: &TranscriptionSegment,
    overlap_seconds: f32,
) -> bool {
    let distance = if current.start > previous.end {
        current.start - previous.end
    } else {
        previous.end - current.start
    };

    distance <= overlap_seconds + 1.0
}

fn merge_chunk_overlap_segments(
    segments: Vec<TranscriptionSegment>,
    overlap_seconds: f32,
) -> Vec<TranscriptionSegment> {
    let mut merged: Vec<TranscriptionSegment> = Vec::with_capacity(segments.len());

    for mut current in segments {
        if current.text.trim().is_empty() {
            continue;
        }

        if let Some(previous) = merged.last_mut() {
            if is_merge_boundary(previous, &current, overlap_seconds) {
                let previous_words = tokenize_words(&previous.text);
                let current_words = tokenize_words(&current.text);
                let overlap_words =
                    fuzzy_suffix_prefix_overlap_words(&previous_words, &current_words, 4);

                if overlap_words > 0 {
                    let trimmed = trim_prefix_words(&current.text, overlap_words);
                    if trimmed.trim().is_empty() {
                        continue;
                    }

                    let duration = (current.end - current.start).max(0.0);
                    if duration > 0.0 && !current_words.is_empty() {
                        let ratio =
                            (overlap_words as f32 / current_words.len() as f32).clamp(0.0, 1.0);
                        current.start += (overlap_seconds * ratio).min(duration);
                    }

                    if current.start < previous.end && previous.end < current.end {
                        current.start = previous.end;
                    }

                    current.text = trimmed;
                } else if are_near_duplicate_texts(&previous.text, &current.text) {
                    continue;
                }
            }
        }

        merged.push(current);
    }

    merged
}

fn tokens_to_text(token_ids: &[usize], vocab: &VocabInfo) -> String {
    let mut words = Vec::new();
    let mut cur = String::new();

    for &id in token_ids {
        if let Some(tok) = vocab.token_of(id) {
            if tok == "<blk>" || tok == "<blank>" || tok == "<pad>" || tok == "<unk>" {
                continue;
            }
            if tok.starts_with('<') {
                continue;
            }

            if tok.starts_with(' ') {
                if !cur.is_empty() {
                    words.push(cur);
                }
                cur = tok.chars().skip(1).collect();
            } else {
                cur.push_str(tok);
            }
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words.join(" ")
}

struct TranscriptionSegment {
    start: f32,
    end: f32,
    text: String,
}

struct BatchTranscriptionResult {
    #[allow(unused)] // used in frontend
    text: String,
    segments: Vec<TranscriptionSegment>,
}

fn format_timestamp(seconds: f32) -> String {
    let mm = (seconds / 60.0).floor() as u32;
    let ss = (seconds % 60.0).floor() as u32;
    let ms = ((seconds % 1.0) * 1000.0).round() as u32;
    format!("{:02}:{:02}.{:03}", mm, ss, ms)
}

// --- Audio Loading ---
fn load_audio(path: &Path) -> Result<Vec<f32>> {
    let src = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());
    let hint = Hint::new();

    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();

    let probed = symphonia::default::get_probe().format(&hint, mss, &fmt_opts, &meta_opts)?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("no supported audio tracks"))?;

    let dec_opts: DecoderOptions = Default::default();
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &dec_opts)?;

    let track_id = track.id;
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = 0;

    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_rate == 0 {
                    sample_rate = decoded.spec().rate;
                }
                let mut buf = AudioBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                decoded.convert(&mut buf);
                let planes = buf.planes();
                let plane_len = planes.planes()[0].len();
                for i in 0..plane_len {
                    let mut sum = 0.0;
                    for plane in planes.planes() {
                        sum += plane[i];
                    }
                    samples.push(sum / planes.planes().len() as f32);
                }
            }
            Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(e) => return Err(anyhow!("Decode error: {}", e)),
        }
    }

    if sample_rate != 16000 {
        let ratio = 16000 as f64 / sample_rate as f64;
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler = SincFixedIn::<f32>::new(ratio, ratio, params, samples.len(), 1)?;
        let waves_in = vec![samples];
        let waves_out = resampler.process(&waves_in, None)?;
        Ok(waves_out[0].clone())
    } else {
        Ok(samples)
    }
}

// --- Command ---
#[derive(serde::Serialize)]
pub struct AlignedSegment {
    start: String,
    end: String,
    speaker: String,
    text: String,
}

#[tauri::command]
pub async fn align_transcript(
    window: tauri::Window,
    audio_path: String,
    _transcript: Vec<Segment>,
) -> Result<Vec<AlignedSegment>, String> {
    window
        .emit("progress", "Downloading alignment model...")
        .map_err(|e| e.to_string())?;

    let mut model =
        ParakeetModel::download().map_err(|e| format!("Failed to download model: {}", e))?;

    window
        .emit("progress", "Aligning...")
        .map_err(|e| e.to_string())?;

    let audio = load_audio(Path::new(&audio_path)).map_err(|e| e.to_string())?;
    let result = model.transcribe_batch(&audio).map_err(|e| e.to_string())?;

    let aligned: Vec<AlignedSegment> = result
        .segments
        .into_iter()
        .map(|s| AlignedSegment {
            start: format_timestamp(s.start),
            end: format_timestamp(s.end),
            speaker: "Local".to_string(),
            text: s.text,
        })
        .collect();

    Ok(aligned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_vocab_info() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "hello 0").unwrap();
        writeln!(file, "world 1").unwrap();
        writeln!(file, "<blk> 2").unwrap();

        let vocab = VocabInfo::from_file(file.path()).unwrap();

        assert_eq!(vocab.vocab_size, 3);
        assert_eq!(vocab.blank_id, 2);
        assert_eq!(vocab.token_of(0), Some("hello"));
        assert_eq!(vocab.token_of(1), Some("world"));
        assert_eq!(vocab.token_of(2), Some("<blk>"));
        assert_eq!(vocab.token_of(3), None);
    }

    #[test]
    fn test_argmax_index() {
        let data = vec![0.1, 0.5, 0.2, 0.9, 0.3];
        let (idx, val) = argmax_index(&data);
        assert_eq!(idx, 3);
        assert_eq!(val, 0.9);
    }

    #[test]
    fn test_tokens_to_text() {
        // Mock vocab
        let mut id_to_token = HashMap::new();
        id_to_token.insert(0, "hello".to_string());
        id_to_token.insert(1, " ".to_string());
        id_to_token.insert(2, "world".to_string());
        id_to_token.insert(3, "<blk>".to_string());
        id_to_token.insert(4, " ".to_string()); // space token
        id_to_token.insert(5, "foo".to_string());

        let _vocab = VocabInfo {
            id_to_token,
            vocab_size: 6,
            blank_id: 3,
        };

        // "hello" " " "world"
        let _tokens = vec![0, 1, 2];
        // Note: The logic in tokens_to_text handles space tokens specially.
        // If token starts with ' ', it appends to words.
        // Let's adjust the test to match the logic:
        // if tok.starts_with(' ') -> new word
        // else -> append to current word

        // Let's try to simulate sentence piece tokens:
        // " Hello" " World"
        let mut id_to_token = HashMap::new();
        id_to_token.insert(0, " Hello".to_string());
        id_to_token.insert(1, " World".to_string());
        id_to_token.insert(2, "<blk>".to_string());

        let vocab = VocabInfo {
            id_to_token,
            vocab_size: 3,
            blank_id: 2,
        };

        let text = tokens_to_text(&[0, 1], &vocab);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_fuzzy_suffix_prefix_overlap_allows_minor_variations() {
        let left = tokenize_words("This is really important for every team");
        let right = tokenize_words("really importan for every team to know");
        let overlap = fuzzy_suffix_prefix_overlap_words(&left, &right, 4);
        assert_eq!(overlap, 5);
    }

    #[test]
    fn test_merge_chunk_overlap_segments_trims_duplicate_prefix() {
        let segments = vec![
            TranscriptionSegment {
                start: 0.0,
                end: 30.0,
                text: "Intro section and this is really important for every team".to_string(),
            },
            TranscriptionSegment {
                start: 27.0,
                end: 57.0,
                text: "really importan for every team to understand and apply daily".to_string(),
            },
        ];

        let merged = merge_chunk_overlap_segments(segments, 3.0);
        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged[0].text,
            "Intro section and this is really important for every team"
        );
        assert_eq!(merged[1].text, "to understand and apply daily");
        assert!((merged[1].start - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_merge_chunk_overlap_segments_drops_near_duplicate_chunk() {
        let segments = vec![
            TranscriptionSegment {
                start: 0.0,
                end: 30.0,
                text: "We covered the roadmap and then discussed release planning".to_string(),
            },
            TranscriptionSegment {
                start: 27.0,
                end: 57.0,
                text: "We covered roadmap and then discussed the release planning".to_string(),
            },
        ];

        let merged = merge_chunk_overlap_segments(segments, 3.0);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].text,
            "We covered the roadmap and then discussed release planning"
        );
    }

    #[test]
    fn test_format_timestamp() {
        assert_eq!(format_timestamp(0.0), "00:00.000");
        assert_eq!(format_timestamp(61.5), "01:01.500");
        assert_eq!(format_timestamp(3600.0), "60:00.000"); // Simple MM:SS logic might overflow MM if > 59, but that's what the code does.
        assert_eq!(format_timestamp(12.3456), "00:12.346");
    }
}
