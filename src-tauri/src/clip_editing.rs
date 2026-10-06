//! Auto editing of clips (shorts): an LLM reads a clip's words and proposes
//! what to cut so it flows better: false starts, repetitions,
//! self-corrections, asides. The proposals become ordinary cuts on the
//! clip, which the user reviews like their own; the transcript is never
//! changed.
//!
//! Like clip selection, the model answers with word numbers under a JSON
//! schema rather than with text or times, and every answer is validated:
//! out-of-range or reversed ranges are dropped, overlaps merged, and the
//! total is capped so a clip can't be gutted.

use crate::gemini::GeminiClient;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// At most this share of a clip's words is cut.
const MAX_CUT_SHARE: f64 = 0.35;
/// The clip's first words (the hook) are never cut, unless the cut is only
/// throat-clearing before it: a cut may start at the first word but must
/// end before this many.
const HOOK_WORDS: usize = 4;
/// Clips edited at once.
const CONCURRENCY: usize = 3;

/// One word of a clip, in playback order.
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EditWord {
    pub text: String,
    pub speaker: String,
    /// The first word after a splice (a jump to another moment).
    pub splice: bool,
}

/// A clip to edit.
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EditClip {
    pub title: String,
    /// Looped shorts: the ending leads back into the opening, so both stay.
    pub looped: bool,
    pub words: Vec<EditWord>,
}

/// Words `from..=to` (indices into the clip's words) to cut, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct WordCut {
    pub from: u32,
    pub to: u32,
    pub reason: String,
}

const SYSTEM_PROMPT: &str = "You are an experienced short-form video editor. You tighten clips \
for TikTok, Reels and YouTube Shorts by cutting words out of what people said, the way an editor \
cuts the video: you never rewrite, reorder or add anything. You only select numbered words to \
remove.";

fn user_prompt(clip: &EditClip) -> String {
    let mut prompt = String::from(
        "Make this clip flow better by cutting words out of it. The words are numbered; each \
         cut removes a range of them, so the video jumps over that part.\n\n\
         Cut:\n\
         - false starts and restarted sentences (keep the version that is finished)\n\
         - repeated words and phrases, and self-corrections (keep the corrected version)\n\
         - verbal tics and filler phrases (\"you know\", \"I mean\", \"sozusagen\", \"ich sag mal\", \
           \"genau\" used as filler)\n\
         - asides, tangents and restatements that don't serve the clip's point\n\n\
         Rules:\n\
         - What remains must read as natural, grammatical speech in the original language and \
           keep the meaning. Cut whole phrases at natural boundaries; never leave a sentence \
           broken.\n\
         - Keep the opening line (the hook) and the payoff. Only throat-clearing before the \
           hook may go.\n\
         - Single filler sounds (uh, um, äh, ähm) are removed automatically; don't list them on \
           their own.\n\
         - Cut at most a third of the words. If the clip already flows, return no cuts.\n\
         - List cuts in order of importance, most valuable first, and give a short reason for \
           each in the transcript's language.\n",
    );
    if clip.looped {
        prompt.push_str(
            "- This is a LOOPED short: its last sentence leads back into its first. Keep the \
             ending and the opening intact.\n",
        );
    }
    prompt.push_str(&format!("\nClip: {}\n\nWords:\n", clip.title.trim()));
    let mut speaker: Option<&str> = None;
    for (index, word) in clip.words.iter().enumerate() {
        if word.splice {
            prompt.push_str("\n[cut to another moment]");
            speaker = None;
        }
        if speaker != Some(word.speaker.as_str()) {
            prompt.push_str(&format!("\n{}:", word.speaker));
            speaker = Some(&word.speaker);
        }
        prompt.push_str(&format!(" {index}:{}", word.text));
    }
    prompt.push('\n');
    prompt
}

fn response_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "cuts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "from": { "type": "integer", "description": "First word number to cut" },
                        "to": { "type": "integer", "description": "Last word number to cut, inclusive" },
                        "reason": { "type": "string", "description": "Why, in a few words" }
                    },
                    "required": ["from", "to", "reason"]
                }
            }
        },
        "required": ["cuts"]
    })
}

#[derive(Deserialize)]
struct RawResponse {
    cuts: Vec<RawCut>,
}

#[derive(Deserialize)]
struct RawCut {
    from: i64,
    to: i64,
    #[serde(default)]
    reason: String,
}

/// The model's cuts for a clip of `count` words, validated: in range, the
/// hook kept, overlaps merged, and at most `MAX_CUT_SHARE` of the words (in
/// the model's order of importance). Sorted by position.
fn parse_cuts(response: &str, count: usize) -> Result<Vec<WordCut>> {
    let json = match (response.find('{'), response.rfind('}')) {
        (Some(start), Some(end)) if start < end => &response[start..=end],
        _ => return Err(anyhow!("The edit response contained no JSON object")),
    };
    let raw: RawResponse =
        serde_json::from_str(json).context("The edit response did not match the schema")?;
    let budget = (count as f64 * MAX_CUT_SHARE).floor() as usize;
    let mut cut = vec![false; count];
    let mut kept: Vec<WordCut> = Vec::new();
    for raw in raw.cuts {
        let (Ok(from), Ok(to)) = (usize::try_from(raw.from), usize::try_from(raw.to)) else {
            continue;
        };
        if from > to || to >= count {
            log::warn!("Dropping edit {from}-{to}: outside the clip's {count} words");
            continue;
        }
        // Into the hook: only a cut of what comes before it, from the start.
        if from < HOOK_WORDS && !(from == 0 && to < HOOK_WORDS - 1) {
            log::warn!("Dropping edit {from}-{to}: it cuts into the hook");
            continue;
        }
        let new = (from..=to).filter(|&i| !cut[i]).count();
        let total = cut.iter().filter(|&&c| c).count();
        if total + new > budget {
            log::warn!("Dropping edit {from}-{to}: it would cut more than a third");
            continue;
        }
        cut[from..=to].iter_mut().for_each(|c| *c = true);
        kept.push(WordCut {
            from: from as u32,
            to: to as u32,
            reason: raw.reason.trim().to_string(),
        });
    }
    // Merge overlapping or touching cuts, keeping their reasons.
    kept.sort_by_key(|cut| cut.from);
    let mut merged: Vec<WordCut> = Vec::new();
    for cut in kept {
        match merged.last_mut() {
            Some(last) if cut.from <= last.to + 1 => {
                last.to = last.to.max(cut.to);
                if !cut.reason.is_empty() && !last.reason.contains(&cut.reason) {
                    last.reason = format!("{}; {}", last.reason, cut.reason);
                }
            }
            _ => merged.push(cut),
        }
    }
    Ok(merged)
}

/// Proposed cuts for each clip, in order. A clip whose request fails gets
/// none (logged) rather than failing the others; if every clip fails, the
/// first error is returned.
pub async fn suggest_cuts(
    client: &GeminiClient,
    clips: &[EditClip],
    on_progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<Vec<WordCut>>> {
    let schema = response_schema();
    let limit = tokio::sync::Semaphore::new(CONCURRENCY);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = clips.len();
    let requests = clips.iter().map(|clip| {
        let (schema, limit, done) = (&schema, &limit, &done);
        async move {
            if clip.words.is_empty() {
                return Ok(Vec::new());
            }
            let _permit = limit.acquire().await?;
            let response = client
                .request_structured(SYSTEM_PROMPT, &user_prompt(clip), "clip_edit", schema)
                .await;
            let finished = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            on_progress(finished, total);
            parse_cuts(&response?, clip.words.len())
        }
    });
    let results = futures_util::future::join_all(requests).await;
    if !results.is_empty() && results.iter().all(Result::is_err) {
        return Err(results
            .into_iter()
            .find_map(Result::err)
            .expect("all failed"));
    }
    Ok(results
        .into_iter()
        .zip(clips)
        .map(|(result, clip)| {
            result.unwrap_or_else(|error| {
                log::warn!("No auto edit for '{}': {error:#}", clip.title);
                Vec::new()
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(text: &str, looped: bool) -> EditClip {
        EditClip {
            title: "A clip".into(),
            looped,
            words: text
                .split_whitespace()
                .enumerate()
                .map(|(i, word)| EditWord {
                    text: word.into(),
                    speaker: if i < 6 { "Host" } else { "Guest" }.into(),
                    splice: i == 8,
                })
                .collect(),
        }
    }

    #[test]
    fn the_prompt_numbers_words_by_speaker_and_marks_splices_and_loops() {
        let prompt = user_prompt(&clip("a b c d e f g h i j", true));
        assert!(
            prompt.contains("\nHost: 0:a 1:b 2:c 3:d 4:e 5:f"),
            "{prompt}"
        );
        assert!(prompt.contains("\nGuest: 6:g 7:h\n[cut to another moment]\nGuest: 8:i 9:j"));
        assert!(prompt.contains("LOOPED"));
        assert!(!user_prompt(&clip("a b", false)).contains("LOOPED"));
    }

    fn response(cuts: Value) -> String {
        json!({ "cuts": cuts }).to_string()
    }

    #[test]
    fn cuts_are_validated_merged_and_capped() {
        // 20 words: at most 7 may go.
        let cuts = parse_cuts(
            &response(json!([
                { "from": 10, "to": 12, "reason": "Fehlstart" },
                { "from": 12, "to": 13, "reason": "Wiederholung" },   // overlaps: merged
                { "from": 2, "to": 5, "reason": "into the hook" },     // dropped
                { "from": 0, "to": 1, "reason": "Räuspern" },          // before the hook: kept
                { "from": 15, "to": 99, "reason": "out of range" },    // dropped
                { "from": 9, "to": 7, "reason": "reversed" },          // dropped
                { "from": 16, "to": 19, "reason": "too much" },        // over the budget: dropped
            ])),
            20,
        )
        .unwrap();
        assert_eq!(
            cuts,
            [
                WordCut {
                    from: 0,
                    to: 1,
                    reason: "Räuspern".into()
                },
                WordCut {
                    from: 10,
                    to: 13,
                    reason: "Fehlstart; Wiederholung".into()
                },
            ]
        );
    }

    #[test]
    fn no_cuts_is_a_fine_answer_and_garbage_is_an_error() {
        assert!(parse_cuts(&response(json!([])), 10).unwrap().is_empty());
        assert!(parse_cuts("no json here", 10).is_err());
    }

    #[tokio::test]
    async fn every_clip_is_edited_against_a_mock_llm() {
        let mut server = mockito::Server::new_async().await;
        let body = json!({
            "choices": [{ "message": { "content": response(json!([
                { "from": 5, "to": 6, "reason": "Wiederholung" }
            ])) } }]
        });
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "response_format": { "type": "json_schema", "json_schema": { "name": "clip_edit" } }
            })))
            .with_body(body.to_string())
            .expect(2)
            .create_async()
            .await;
        let client = GeminiClient::new("key".into(), server.url(), "model".into());
        let clips = [
            clip("a b c d e f g h i j", false),
            clip("k l m n o p q r s t", false),
            clip("", false),
        ];
        let progress = std::sync::Mutex::new(Vec::new());
        let cuts = suggest_cuts(&client, &clips, &|done, total| {
            progress.lock().unwrap().push((done, total))
        })
        .await
        .unwrap();
        mock.assert_async().await;
        assert_eq!(cuts.len(), 3);
        assert_eq!(
            cuts[0],
            [WordCut {
                from: 5,
                to: 6,
                reason: "Wiederholung".into()
            }]
        );
        assert!(cuts[2].is_empty());
        assert_eq!(progress.lock().unwrap().len(), 2);
    }
}
