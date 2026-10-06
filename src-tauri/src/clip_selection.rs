//! Picking short-form clips out of a transcript (shorts plan, phase S1).
//!
//! The LLM sees the transcript as numbered lines (`S12 [03:21-03:27] Speaker 1:
//! ...`) and answers, under a JSON schema, with segment IDs rather than
//! timestamps. Every clip therefore starts and ends on a real segment
//! boundary, and a made-up timestamp can't occur; the one exception is the
//! opening, which moves onto the hook line's first word when the segment
//! opens with the end of the sentence before. Responses are validated;
//! anything malformed is dropped rather than guessed at.
//!
//! Long transcripts are handled map-reduce style: overlapping windows are
//! analysed independently and the candidates are ranked and de-duplicated
//! locally.

use crate::gemini::GeminiClient;
use crate::time_utils::parse_timestamp_to_seconds_raw;
use crate::video::TranscriptSegment;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Window length and overlap for long transcripts.
const WINDOW_SECONDS: f64 = 600.0;
const WINDOW_OVERLAP_SECONDS: f64 = 60.0;
/// Transcripts up to this long are analysed in one request.
const SINGLE_WINDOW_MAX_SECONDS: f64 = 900.0;
/// Durations may miss the requested bounds by this fraction.
const DURATION_TOLERANCE: f64 = 0.15;
/// Two candidates overlapping by more than this share of the shorter one are
/// treated as the same moment.
const MAX_OVERLAP_SHARE: f64 = 0.3;

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipRequest {
    pub count: u32,
    pub min_seconds: f64,
    pub max_seconds: f64,
    pub topic: Option<String>,
    /// Allow clips made of several non-contiguous ranges (e.g. a cold open).
    pub allow_splicing: bool,
    /// Looped shorts: the opener is the story's resolution and the closing
    /// line leads back into it.
    pub looped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ClipRole {
    Hook,
    Body,
    Payoff,
    LoopOpener,
    Closing,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipRange {
    #[specta(type = specta_typescript::Number)]
    pub start: f64,
    #[specta(type = specta_typescript::Number)]
    pub end: f64,
    pub role: ClipRole,
    /// Index of the first and last transcript segment in the range.
    pub first_segment: u32,
    pub last_segment: u32,
}

/// The LLM's 1-10 ratings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipRatings {
    pub hook: u8,
    pub standalone: u8,
    pub emotion: u8,
    pub info: u8,
    /// How well the closing leads back into the opener (looped clips only).
    pub loop_continuity: u8,
}

/// Cheap signals measured from the transcript itself.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipSignals {
    pub speaker_changes: u32,
    pub laughs: u32,
    #[specta(type = specta_typescript::Number)]
    pub words_per_second: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ClipCandidate {
    pub title: String,
    pub hook_line: String,
    pub reason: String,
    /// In playback order.
    pub ranges: Vec<ClipRange>,
    pub ratings: ClipRatings,
    pub signals: ClipSignals,
    /// Overall rank score, 0-100.
    #[specta(type = specta_typescript::Number)]
    pub score: f64,
    #[specta(type = specta_typescript::Number)]
    pub duration: f64,
    pub looped: bool,
}

/// A transcript segment with parsed times, addressed by its index.
#[derive(Debug, Clone)]
pub(crate) struct Line {
    index: usize,
    start: f64,
    end: f64,
    speaker: String,
    text: String,
    word_count: usize,
    /// Start time and text of each timed word.
    words: Vec<(f64, String)>,
}

pub(crate) fn lines_of(transcript: &[TranscriptSegment]) -> Result<Vec<Line>> {
    transcript
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let start = parse_timestamp_to_seconds_raw(&segment.start)
                .with_context(|| format!("Segment {index} has an invalid start time"))?;
            let end = parse_timestamp_to_seconds_raw(&segment.end)
                .with_context(|| format!("Segment {index} has an invalid end time"))?;
            Ok(Line {
                index,
                start,
                end: end.max(start),
                speaker: segment.speaker.clone(),
                text: segment.text.trim().to_string(),
                word_count: segment
                    .words
                    .as_ref()
                    .map(Vec::len)
                    .unwrap_or_else(|| segment.text.split_whitespace().count()),
                words: segment
                    .words
                    .iter()
                    .flatten()
                    .filter_map(|word| {
                        let start = parse_timestamp_to_seconds_raw(&word.start).ok()?;
                        Some((start, word.text.clone()))
                    })
                    .collect(),
            })
        })
        .collect()
}

fn mmss(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn render_lines(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| {
            format!(
                "S{} [{}-{}] {}: {}",
                line.index,
                mmss(line.start),
                mmss(line.end),
                line.speaker,
                line.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split into overlapping windows of whole segments; short transcripts stay
/// whole.
pub(crate) fn windows(lines: &[Line]) -> Vec<&[Line]> {
    let Some(last) = lines.last() else {
        return Vec::new();
    };
    let total = last.end - lines[0].start;
    if total <= SINGLE_WINDOW_MAX_SECONDS {
        return vec![lines];
    }

    let mut result = Vec::new();
    let mut window_start = lines[0].start;
    loop {
        let window_end = window_start + WINDOW_SECONDS;
        let first = lines.partition_point(|line| line.end <= window_start);
        let last = lines.partition_point(|line| line.start < window_end);
        if first < last {
            result.push(&lines[first..last]);
        }
        if window_end >= last_end(lines) {
            break;
        }
        window_start = window_end - WINDOW_OVERLAP_SECONDS;
    }
    result
}

fn last_end(lines: &[Line]) -> f64 {
    lines.last().map_or(0.0, |line| line.end)
}

const SYSTEM_PROMPT: &str = "You are an experienced short-form video editor. You find the moments \
in long recordings that work as standalone vertical shorts (TikTok, Reels, YouTube Shorts): a \
strong hook in the first seconds, one complete idea, and a satisfying payoff. You never invent \
content; you only select transcript segments by their IDs.";

pub(crate) fn user_prompt(window: &[Line], request: &ClipRequest, count: u32) -> String {
    let mut prompt = format!(
        "Select up to {count} clips from the transcript below.\n\n\
         Rules:\n\
         - Refer to transcript segments only by their IDs (e.g. \"S12\"). A range runs from its \
           `from` segment to its `to` segment, inclusive.\n\
         - Each clip must last between {min:.0} and {max:.0} seconds in total (use the timestamps).\n\
         - A clip must make sense to someone who hasn't seen the rest of the recording.\n\
         - Clips must not repeat the same moment.\n\
         - Rate each clip 1-10: hook (do the first seconds grab attention?), standalone (does it \
           work without context?), emotion (humour, surprise, conflict, passion), info (is it \
           useful or insightful?).\n",
        min = request.min_seconds,
        max = request.max_seconds,
    );
    if request.looped {
        prompt.push_str(
            "- These are LOOPED shorts. Each clip has three parts: a `loop_opener` range that is \
             the resolution of the story (a conclusion, punchline or reveal) and still works as a \
             hook on its own; then `body` ranges building towards it; then a `closing` range whose \
             last sentence leads naturally back into the opener, so the replay continues the \
             thought. The opener usually comes from later in the recording than the body. Rate \
             loop_continuity 1-10 for how seamlessly closing -> opener reads.\n",
        );
    } else if request.allow_splicing {
        prompt.push_str(
            "- A clip may combine non-contiguous ranges into one coherent story, e.g. a strong \
             line from later as a `hook` cold open before the build-up. Mark ranges as `hook`, \
             `body` or `payoff`. Set loop_continuity to 0.\n",
        );
    } else {
        prompt.push_str(
            "- Each clip is exactly one contiguous range with role `body`. Set loop_continuity \
             to 0.\n",
        );
    }
    if let Some(topic) = request.topic.as_deref().filter(|t| !t.trim().is_empty()) {
        prompt.push_str(&format!("- Focus on this topic: {}\n", topic.trim()));
    }
    prompt.push_str("\nTranscript:\n");
    prompt.push_str(&render_lines(window));
    prompt
}

pub(crate) fn response_schema() -> Value {
    let rating = json!({ "type": "integer", "minimum": 0, "maximum": 10 });
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "clips": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "title": { "type": "string", "description": "Catchy title, under 60 characters" },
                        "hook_line": { "type": "string", "description": "The first sentence the viewer hears" },
                        "reason": { "type": "string", "description": "Why this works as a short" },
                        "ranges": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "from": { "type": "string", "description": "First segment ID, e.g. S12" },
                                    "to": { "type": "string", "description": "Last segment ID, inclusive" },
                                    "role": {
                                        "type": "string",
                                        "enum": ["hook", "body", "payoff", "loop_opener", "closing"]
                                    }
                                },
                                "required": ["from", "to", "role"]
                            }
                        },
                        "ratings": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "hook": rating,
                                "standalone": rating,
                                "emotion": rating,
                                "info": rating,
                                "loop_continuity": rating
                            },
                            "required": ["hook", "standalone", "emotion", "info", "loop_continuity"]
                        }
                    },
                    "required": ["title", "hook_line", "reason", "ranges", "ratings"]
                }
            }
        },
        "required": ["clips"]
    })
}

#[derive(Deserialize)]
struct RawResponse {
    clips: Vec<RawClip>,
}

#[derive(Deserialize)]
struct RawClip {
    title: String,
    #[serde(default)]
    hook_line: String,
    #[serde(default)]
    reason: String,
    ranges: Vec<RawRange>,
    ratings: RawRatings,
}

#[derive(Deserialize)]
struct RawRange {
    from: String,
    to: String,
    role: ClipRole,
}

#[derive(Deserialize)]
struct RawRatings {
    hook: u8,
    standalone: u8,
    emotion: u8,
    info: u8,
    #[serde(default)]
    loop_continuity: u8,
}

fn segment_index(id: &str) -> Option<usize> {
    id.trim()
        .trim_start_matches(['S', 's'])
        .parse::<usize>()
        .ok()
}

/// Turn an LLM response into validated candidates. Invalid clips are skipped
/// (and logged) so one bad clip doesn't cost the whole response.
pub(crate) fn parse_candidates(
    response: &str,
    lines: &[Line],
    request: &ClipRequest,
) -> Result<Vec<ClipCandidate>> {
    let json = match (response.find('{'), response.rfind('}')) {
        (Some(start), Some(end)) if start < end => &response[start..=end],
        _ => return Err(anyhow!("The clip response contained no JSON object")),
    };
    let raw: RawResponse =
        serde_json::from_str(json).context("The clip response did not match the schema")?;

    Ok(raw
        .clips
        .into_iter()
        .filter_map(|clip| {
            let title = clip.title.clone();
            match validate(clip, lines, request) {
                Ok(candidate) => Some(candidate),
                Err(error) => {
                    log::warn!("Dropping clip '{title}': {error:#}");
                    None
                }
            }
        })
        .collect())
}

fn validate(clip: RawClip, lines: &[Line], request: &ClipRequest) -> Result<ClipCandidate> {
    if clip.ranges.is_empty() {
        return Err(anyhow!("no ranges"));
    }

    let mut ranges = Vec::with_capacity(clip.ranges.len());
    for range in &clip.ranges {
        let (first, last) = segment_index(&range.from)
            .zip(segment_index(&range.to))
            .ok_or_else(|| anyhow!("unknown segment ID {} or {}", range.from, range.to))?;
        if first > last || last >= lines.len() {
            return Err(anyhow!("invalid range {}-{}", range.from, range.to));
        }
        ranges.push(ClipRange {
            start: lines[first].start,
            end: lines[last].end,
            role: range.role,
            first_segment: first as u32,
            last_segment: last as u32,
        });
    }

    // No moment may play twice.
    let mut by_source: Vec<&ClipRange> = ranges.iter().collect();
    by_source.sort_by_key(|range| range.first_segment);
    if by_source
        .windows(2)
        .any(|pair| pair[1].first_segment <= pair[0].last_segment)
    {
        return Err(anyhow!("ranges overlap"));
    }

    if request.looped {
        let roles: Vec<ClipRole> = ranges.iter().map(|range| range.role).collect();
        if roles.first() != Some(&ClipRole::LoopOpener) || roles.last() != Some(&ClipRole::Closing)
        {
            return Err(anyhow!(
                "a looped clip must start with the loop opener and end with the closing"
            ));
        }
    } else if !request.allow_splicing {
        let contiguous = by_source
            .windows(2)
            .all(|pair| pair[1].first_segment == pair[0].last_segment + 1);
        if !contiguous {
            return Err(anyhow!("splicing is off but the clip has separate ranges"));
        }
        let first = by_source[0].first_segment as usize;
        let last = by_source[by_source.len() - 1].last_segment as usize;
        ranges = vec![ClipRange {
            start: lines[first].start,
            end: lines[last].end,
            role: ClipRole::Body,
            first_segment: first as u32,
            last_segment: last as u32,
        }];
    }

    align_to_hook(&mut ranges, lines, &clip.hook_line);

    let duration: f64 = ranges.iter().map(|range| range.end - range.start).sum();
    let (min, max) = (
        request.min_seconds * (1.0 - DURATION_TOLERANCE),
        request.max_seconds * (1.0 + DURATION_TOLERANCE),
    );
    if duration < min || duration > max {
        return Err(anyhow!(
            "lasts {duration:.0}s, outside {:.0}-{:.0}s",
            request.min_seconds,
            request.max_seconds
        ));
    }

    let clamp = |value: u8| value.min(10);
    let ratings = ClipRatings {
        hook: clamp(clip.ratings.hook),
        standalone: clamp(clip.ratings.standalone),
        emotion: clamp(clip.ratings.emotion),
        info: clamp(clip.ratings.info),
        loop_continuity: if request.looped {
            clamp(clip.ratings.loop_continuity)
        } else {
            0
        },
    };
    let signals = signals(&ranges, lines, duration);
    let score = score(&ratings, &signals, request.looped);

    Ok(ClipCandidate {
        title: clip.title.trim().to_string(),
        hook_line: clip.hook_line.trim().to_string(),
        reason: clip.reason.trim().to_string(),
        ranges,
        ratings,
        signals,
        score,
        duration,
        looped: request.looped,
    })
}

/// Hook words that must match in a row to find the hook in a segment.
const HOOK_MATCH_WORDS: usize = 3;

/// A word compared loosely: lower case, letters and digits only.
fn loose(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Leading hook words that may differ from the transcript (the LLM writes
/// "60%" where the transcript says "60 Prozent").
const HOOK_SKIP_WORDS: usize = 2;

/// Where `hook` starts inside `line` (seconds), if it starts after the
/// line's first word: `HOOK_MATCH_WORDS` of its first words found in a row
/// among the line's, if need be from its second or third word on (then the
/// start goes back to its first word, if that is near).
fn hook_start(line: &Line, hook: &str) -> Option<f64> {
    let hook: Vec<String> = hook
        .split_whitespace()
        .map(loose)
        .filter(|w| !w.is_empty())
        .collect();
    let words: Vec<(f64, String)> = line
        .words
        .iter()
        .map(|(start, text)| (*start, loose(text)))
        .filter(|(_, w)| !w.is_empty())
        .collect();
    for skip in 0..=HOOK_SKIP_WORDS.min(hook.len().saturating_sub(1)) {
        let rest = &hook[skip..];
        let need = rest.len().min(HOOK_MATCH_WORDS);
        if need == 0 || (skip > 0 && need < HOOK_MATCH_WORDS) || words.len() < need {
            break;
        }
        let Some(found) = (0..=words.len().saturating_sub(need)).find(|&k| {
            words[k..k + need]
                .iter()
                .zip(rest)
                .all(|((_, word), hook)| word == hook)
        }) else {
            continue;
        };
        // Back to where the hook's first word is, else as many words back
        // as were skipped.
        let first = (found.saturating_sub(skip + 2)..=found)
            .rev()
            .find(|&k| words[k].1.starts_with(&hook[0]))
            .unwrap_or(found.saturating_sub(skip));
        return (first > 0).then(|| words[first].0);
    }
    None
}

/// Segments are transcription chunks, not sentences, so the first one may
/// open with the end of the previous sentence. Where the opening segment
/// contains the hook line after its start, the clip starts on the hook; the
/// skipped words go to the range that ends where the opening segment starts
/// (a looped clip's closing, which then leads into the hook), or are cut.
fn align_to_hook(ranges: &mut [ClipRange], lines: &[Line], hook: &str) {
    let Some(first) = ranges.first() else {
        return;
    };
    let segment = first.first_segment;
    let Some(start) = hook_start(&lines[segment as usize], hook) else {
        return;
    };
    if start <= first.start || start >= first.end {
        return;
    }
    ranges[0].start = start;
    if let Some(before) = ranges[1..]
        .iter_mut()
        .find(|range| range.last_segment + 1 == segment)
    {
        before.end = start;
    }
}

fn signals(ranges: &[ClipRange], lines: &[Line], duration: f64) -> ClipSignals {
    let mut speaker_changes = 0;
    let mut laughs = 0;
    let mut words = 0;
    let mut previous_speaker: Option<&str> = None;
    for range in ranges {
        for line in &lines[range.first_segment as usize..=range.last_segment as usize] {
            if previous_speaker.is_some_and(|speaker| speaker != line.speaker) {
                speaker_changes += 1;
            }
            previous_speaker = Some(&line.speaker);
            let lower = line.text.to_lowercase();
            laughs += lower.matches("[laughter]").count()
                + lower.matches("(laughs)").count()
                + lower.matches("[laughs]").count();
            words += line.word_count;
        }
    }
    ClipSignals {
        speaker_changes,
        laughs: laughs as u32,
        words_per_second: if duration > 0.0 {
            words as f64 / duration
        } else {
            0.0
        },
    }
}

/// Rank score, 0-100: mostly the LLM's ratings (hook 35%, standalone 25%,
/// emotion and info 20% each), plus small capped bonuses for what the
/// transcript shows directly: back-and-forth, laughter and a lively pace.
/// Looped clips also earn up to 10 points for loop continuity.
pub(crate) fn score(ratings: &ClipRatings, signals: &ClipSignals, looped: bool) -> f64 {
    let rated = 0.35 * f64::from(ratings.hook)
        + 0.25 * f64::from(ratings.standalone)
        + 0.20 * f64::from(ratings.emotion)
        + 0.20 * f64::from(ratings.info);
    let mut score = rated * 10.0;
    score += f64::from(signals.speaker_changes.min(3));
    score += 2.0 * f64::from(signals.laughs.min(2));
    if (2.2..=3.6).contains(&signals.words_per_second) {
        score += 2.0;
    }
    if looped {
        score = score * 0.9 + f64::from(ratings.loop_continuity);
    }
    score.clamp(0.0, 100.0)
}

fn overlap_share(a: &ClipCandidate, b: &ClipCandidate) -> f64 {
    let overlap: f64 = a
        .ranges
        .iter()
        .flat_map(|x| b.ranges.iter().map(move |y| (x, y)))
        .map(|(x, y)| (x.end.min(y.end) - x.start.max(y.start)).max(0.0))
        .sum();
    overlap / a.duration.min(b.duration).max(f64::EPSILON)
}

/// Best `count` candidates that don't repeat the same moment.
pub(crate) fn pick_best(mut candidates: Vec<ClipCandidate>, count: usize) -> Vec<ClipCandidate> {
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut picked: Vec<ClipCandidate> = Vec::new();
    for candidate in candidates {
        if picked.len() == count {
            break;
        }
        if picked
            .iter()
            .all(|chosen| overlap_share(chosen, &candidate) <= MAX_OVERLAP_SHARE)
        {
            picked.push(candidate);
        }
    }
    picked
}

/// Run the whole selection: one request per window (up to three at once),
/// then local ranking. `on_progress` gets "window i of n" updates.
pub async fn select_clips(
    client: &GeminiClient,
    transcript: &[TranscriptSegment],
    request: &ClipRequest,
    on_progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<ClipCandidate>> {
    if request.count == 0 {
        return Ok(Vec::new());
    }
    if request.min_seconds <= 0.0 || request.max_seconds < request.min_seconds {
        return Err(anyhow!("The clip duration range is invalid"));
    }
    let lines = lines_of(transcript)?;
    let windows = windows(&lines);
    if windows.is_empty() {
        return Ok(Vec::new());
    }
    // Ask each window for a few spare candidates; the ranking picks the best.
    let per_window = (request.count + 2).min(8);
    let schema = response_schema();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = windows.len();

    // Up to three windows in flight at once.
    let limit = tokio::sync::Semaphore::new(3);
    let mut requests = Vec::with_capacity(total);
    for window in &windows {
        let prompt = user_prompt(window, request, per_window);
        let (schema, lines, done, limit) = (&schema, &lines, &done, &limit);
        requests.push(async move {
            let _permit = limit.acquire().await?;
            let response = client
                .request_structured(SYSTEM_PROMPT, &prompt, "clip_selection", schema)
                .await?;
            let finished = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            on_progress(finished, total);
            parse_candidates(&response, lines, request)
        });
    }
    let responses = futures_util::future::try_join_all(requests).await?;

    let picked = pick_best(
        responses.into_iter().flatten().collect(),
        request.count as usize,
    );
    if picked.is_empty() {
        return Err(anyhow!(
            "The model returned no usable clips. Try a wider duration range or a different topic."
        ));
    }
    Ok(picked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: &str, end: &str, speaker: &str, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start: start.into(),
            end: end.into(),
            speaker: speaker.into(),
            text: text.into(),
            ..Default::default()
        }
    }

    fn timed_transcript() -> Vec<TranscriptSegment> {
        (0..12)
            .map(|i| {
                let start = i * 10;
                segment(
                    &format!("{:02}:{:02}", start / 60, start % 60),
                    &format!("{:02}:{:02}", (start + 10) / 60, (start + 10) % 60),
                    if i % 2 == 0 { "Host" } else { "Guest" },
                    &format!("sentence number {i} has seven words here"),
                )
            })
            .collect()
    }

    fn request(looped: bool, allow_splicing: bool) -> ClipRequest {
        ClipRequest {
            count: 3,
            min_seconds: 20.0,
            max_seconds: 40.0,
            topic: None,
            allow_splicing,
            looped,
        }
    }

    fn response(clips: Value) -> String {
        format!("```json\n{}\n```", json!({ "clips": clips }))
    }

    fn clip(ranges: Value, hook: u8) -> Value {
        json!({
            "title": "A title",
            "hook_line": "The hook",
            "reason": "Because",
            "ranges": ranges,
            "ratings": { "hook": hook, "standalone": 7, "emotion": 6, "info": 5, "loop_continuity": 8 }
        })
    }

    /// A segment whose words are spread evenly over it.
    fn worded(start: f64, end: f64, text: &str) -> TranscriptSegment {
        let words: Vec<&str> = text.split_whitespace().collect();
        let step = (end - start) / words.len() as f64;
        let time = |t: f64| format!("{:02}:{:06.3}", (t / 60.0) as u32, t % 60.0);
        TranscriptSegment {
            start: time(start),
            end: time(end),
            speaker: "Annette".into(),
            text: text.into(),
            words: Some(
                words
                    .iter()
                    .enumerate()
                    .map(|(i, word)| crate::video::TranscriptWord {
                        start: time(start + i as f64 * step),
                        end: time(start + (i + 1) as f64 * step),
                        text: (*word).into(),
                        speaker: None,
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    /// The PODIUM case: the opener's segment starts with the end of the
    /// sentence before the hook.
    fn survey_transcript() -> Vec<TranscriptSegment> {
        vec![
            worded(0.0, 10.0, "Wir haben sehr viele Ärzte in den Systemen"),
            worded(10.0, 20.0, "wo das noch mehr genutzt wird. Es gibt eine Umfrage aktuell:"),
            worded(
                20.0,
                30.0,
                "von vor zwei Monaten. 60 Prozent aller Ärzte nutzen Schatten KI einfach als Zugang.",
            ),
        ]
    }

    fn hooked(ranges: Value, hook_line: &str) -> String {
        let mut clip = clip(ranges, 8);
        clip["hook_line"] = json!(hook_line);
        response(json!([clip]))
    }

    #[test]
    fn a_loop_opens_on_its_hook_and_the_closing_keeps_the_words_before_it() {
        let lines = lines_of(&survey_transcript()).unwrap();
        let parsed = parse_candidates(
            &hooked(
                json!([
                    { "from": "S2", "to": "S2", "role": "loop_opener" },
                    { "from": "S0", "to": "S0", "role": "body" },
                    { "from": "S1", "to": "S1", "role": "closing" }
                ]),
                "60 Prozent aller Ärzte nutzen Schatten-KI einfach als Zugang.",
            ),
            &lines,
            &request(true, false),
        )
        .unwrap();
        // "60" is the 5th of 14 words in 20-30 s.
        let hook = 20.0 + 4.0 * 10.0 / 14.0;
        let ranges = &parsed[0].ranges;
        assert!((ranges[0].start - hook).abs() < 1e-3, "{ranges:?}");
        assert_eq!(ranges[0].end, 30.0);
        // "…Es gibt eine Umfrage aktuell: von vor zwei Monaten." -> hook.
        assert!((ranges[2].end - hook).abs() < 1e-3, "{ranges:?}");
        assert_eq!((ranges[1].start, ranges[1].end), (0.0, 10.0));
        assert!((parsed[0].duration - 30.0).abs() < 1e-6);
    }

    #[test]
    fn a_clip_drops_the_fragment_before_its_hook_and_keeps_a_hook_at_the_start() {
        let lines = lines_of(&survey_transcript()).unwrap();
        let mut request = request(false, false);
        request.min_seconds = 15.0;
        let parse = |hook: &str| {
            parse_candidates(
                &hooked(json!([{ "from": "S1", "to": "S2", "role": "body" }]), hook),
                &lines,
                &request,
            )
            .unwrap()
            .remove(0)
            .ranges
        };
        // The hook starts mid-segment: "Es" is the 7th of 11 words in 10-20 s.
        let ranges = parse("Es gibt eine Umfrage aktuell");
        assert!((ranges[0].start - (10.0 + 6.0 * 10.0 / 11.0)).abs() < 1e-3);
        // The LLM's "60%" for the transcript's "60 Prozent".
        let loop_lines = lines_of(&survey_transcript()).unwrap();
        let start = hook_start(&loop_lines[2], "60% aller Ärzte nutzen Schatten-KI").unwrap();
        assert!((start - (20.0 + 4.0 * 10.0 / 14.0)).abs() < 1e-3, "{start}");
        // At the segment start, or not found: unchanged.
        assert_eq!(parse("Wo das noch mehr genutzt wird.")[0].start, 10.0);
        assert_eq!(parse("Something else entirely")[0].start, 10.0);
    }

    #[test]
    fn ids_map_to_segment_boundaries() {
        let lines = lines_of(&timed_transcript()).unwrap();
        let parsed = parse_candidates(
            &response(json!([clip(
                json!([{ "from": "S2", "to": "S4", "role": "body" }]),
                8
            )])),
            &lines,
            &request(false, false),
        )
        .unwrap();

        assert_eq!(parsed.len(), 1);
        let range = &parsed[0].ranges[0];
        assert_eq!((range.start, range.end), (20.0, 50.0));
        assert_eq!((range.first_segment, range.last_segment), (2, 4));
        assert_eq!(parsed[0].duration, 30.0);
        assert_eq!(parsed[0].signals.speaker_changes, 2);
    }

    #[test]
    fn invalid_clips_are_dropped_not_guessed() {
        let lines = lines_of(&timed_transcript()).unwrap();
        let clips = json!([
            clip(json!([{ "from": "S2", "to": "S99", "role": "body" }]), 8), // unknown ID
            clip(json!([{ "from": "S5", "to": "S3", "role": "body" }]), 8),  // reversed
            clip(json!([{ "from": "S1", "to": "S1", "role": "body" }]), 8),  // 10 s, too short
            clip(
                json!([{ "from": "S1", "to": "S3", "role": "body" },
                        { "from": "S3", "to": "S4", "role": "body" }]),
                8
            ), // overlap
            clip(json!([{ "from": "S6", "to": "S8", "role": "body" }]), 8),  // fine
        ]);
        let parsed = parse_candidates(&response(clips), &lines, &request(false, true)).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].ranges[0].first_segment, 6);
    }

    #[test]
    fn without_splicing_only_contiguous_ranges_survive_and_are_merged() {
        let lines = lines_of(&timed_transcript()).unwrap();
        let clips = json!([
            clip(
                json!([{ "from": "S1", "to": "S2", "role": "hook" },
                        { "from": "S3", "to": "S3", "role": "body" }]),
                8
            ),
            clip(
                json!([{ "from": "S8", "to": "S8", "role": "hook" },
                        { "from": "S5", "to": "S6", "role": "body" }]),
                8
            ),
        ]);
        let parsed = parse_candidates(&response(clips), &lines, &request(false, false)).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].ranges.len(), 1);
        assert_eq!(
            (parsed[0].ranges[0].start, parsed[0].ranges[0].end),
            (10.0, 40.0)
        );
        assert_eq!(parsed[0].ranges[0].role, ClipRole::Body);
    }

    #[test]
    fn looped_clips_need_opener_first_and_closing_last() {
        let lines = lines_of(&timed_transcript()).unwrap();
        let good = clip(
            json!([{ "from": "S9", "to": "S9", "role": "loop_opener" },
                   { "from": "S2", "to": "S3", "role": "body" },
                   { "from": "S4", "to": "S4", "role": "closing" }]),
            8,
        );
        let bad = clip(
            json!([{ "from": "S2", "to": "S3", "role": "body" },
                   { "from": "S9", "to": "S9", "role": "loop_opener" }]),
            8,
        );
        let parsed =
            parse_candidates(&response(json!([good, bad])), &lines, &request(true, false)).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].looped);
        assert_eq!(parsed[0].ranges[0].role, ClipRole::LoopOpener);
        assert_eq!(parsed[0].ranges[0].start, 90.0);
        assert_eq!(parsed[0].ratings.loop_continuity, 8);
    }

    #[test]
    fn short_transcripts_stay_whole_and_long_ones_get_overlapping_windows() {
        let lines = lines_of(&timed_transcript()).unwrap();
        assert_eq!(windows(&lines).len(), 1);

        // 40 minutes of 10 s segments.
        let long: Vec<TranscriptSegment> = (0..240)
            .map(|i| {
                let start = i * 10;
                segment(
                    &format!("{:02}:{:02}", start / 60, start % 60),
                    &format!("{:02}:{:02}", (start + 10) / 60, (start + 10) % 60),
                    "Host",
                    "words",
                )
            })
            .collect();
        let lines = lines_of(&long).unwrap();
        let windows = windows(&lines);
        assert_eq!(windows.len(), 5);
        assert_eq!(windows[0].first().unwrap().start, 0.0);
        // Each window starts a minute before the previous one ended.
        assert_eq!(windows[1].first().unwrap().start, 540.0);
        assert_eq!(windows.last().unwrap().last().unwrap().end, 2400.0);
    }

    #[test]
    fn ranking_prefers_better_clips_and_skips_repeats() {
        let lines = lines_of(&timed_transcript()).unwrap();
        let clips = json!([
            clip(json!([{ "from": "S0", "to": "S2", "role": "body" }]), 5),
            clip(json!([{ "from": "S1", "to": "S3", "role": "body" }]), 9), // overlaps the first
            clip(json!([{ "from": "S7", "to": "S9", "role": "body" }]), 7),
        ]);
        let parsed = parse_candidates(&response(clips), &lines, &request(false, false)).unwrap();
        let picked = pick_best(parsed, 3);
        let firsts: Vec<u32> = picked.iter().map(|c| c.ranges[0].first_segment).collect();
        assert_eq!(firsts, [1, 7]);
    }

    #[test]
    fn scores_are_driven_by_ratings_with_small_signal_bonuses() {
        let ratings = |hook| ClipRatings {
            hook,
            standalone: 5,
            emotion: 5,
            info: 5,
            loop_continuity: 0,
        };
        let quiet = ClipSignals {
            speaker_changes: 0,
            laughs: 0,
            words_per_second: 1.0,
        };
        let lively = ClipSignals {
            speaker_changes: 5,
            laughs: 3,
            words_per_second: 3.0,
        };
        assert!(score(&ratings(9), &quiet, false) > score(&ratings(5), &lively, false));
        assert!(score(&ratings(5), &lively, false) > score(&ratings(5), &quiet, false));
        assert!(score(&ratings(10), &lively, false) <= 100.0);
    }

    #[test]
    fn prompt_lists_ids_and_asks_for_loops_when_requested() {
        let lines = lines_of(&timed_transcript()).unwrap();
        let prompt = user_prompt(&lines, &request(true, false), 3);
        assert!(prompt.contains("S3 [00:30-00:40] Guest: sentence number 3"));
        assert!(prompt.contains("loop_opener"));
        assert!(!user_prompt(&lines, &request(false, false), 3).contains("LOOPED"));
    }

    #[tokio::test]
    async fn end_to_end_against_a_mock_llm() {
        let mut server = mockito::Server::new_async().await;
        let body = json!({
            "choices": [{ "message": { "content": json!({ "clips": [
                clip(json!([{ "from": "S2", "to": "S4", "role": "body" }]), 8)
            ]}).to_string() } }]
        });
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "response_format": { "type": "json_schema", "json_schema": { "name": "clip_selection" } }
            })))
            .with_body(body.to_string())
            .create_async()
            .await;

        let client = GeminiClient::new("key".into(), server.url(), "model".into());
        let progress = std::sync::Mutex::new(Vec::new());
        let clips = select_clips(
            &client,
            &timed_transcript(),
            &request(false, false),
            &|done, total| progress.lock().unwrap().push((done, total)),
        )
        .await
        .unwrap();

        mock.assert_async().await;
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].ranges[0].start, 20.0);
        assert_eq!(*progress.lock().unwrap(), [(1, 1)]);
    }
}
