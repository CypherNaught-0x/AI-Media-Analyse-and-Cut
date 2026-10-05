use crate::retry::{rate_limit_wait, retry_with_backoff, RetryConfig, RetryableError};
use crate::video::TranscriptSegment;
use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use reqwest::{
    header::{ACCEPT, ACCEPT_ENCODING},
    Client, RequestBuilder,
};
use serde::Deserialize;
use serde_json::{json, Value};

struct OutputFormat;
const RAW_RESPONSE_PREVIEW_LIMIT: usize = 4_000;

impl OutputFormat {
    fn example() -> String {
        let example = vec![TranscriptSegment {
            start: "00:00".to_string(),
            end: "00:05".to_string(),
            speaker: "Speaker 1".to_string(),
            text: "This is an example sentence.".to_string(),
            words: None,
            alternatives: None,
            merge_status: None,
            active_source: None,
            similarity_score: None,
        }];
        serde_json::to_string(&example).unwrap_or_default()
    }
}

fn transcript_response_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "transcript_segments",
            "strict": true,
            "schema": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "start": {
                            "type": "string",
                            "description": "Segment start timestamp in MM:SS format"
                        },
                        "end": {
                            "type": "string",
                            "description": "Segment end timestamp in MM:SS format"
                        },
                        "speaker": {
                            "type": "string",
                            "description": "Stable speaker label such as Speaker 1, or a real name only when the identity is clearly spoken or strongly inferable from context"
                        },
                        "text": {
                            "type": "string",
                            "description": "Transcript text for this segment"
                        }
                    },
                    "required": ["start", "end", "speaker", "text"]
                }
            }
        }
    })
}

fn cleanup_response_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "cleanup_segments",
            "strict": true,
            "schema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "segments": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "start_index": {
                                    "type": "integer",
                                    "minimum": 0
                                },
                                "end_index": {
                                    "type": "integer",
                                    "minimum": 0
                                },
                                "text": {
                                    "type": "string"
                                },
                                "speaker": {
                                    "type": "string"
                                }
                            },
                            "required": ["start_index", "end_index", "text"]
                        }
                    }
                },
                "required": ["segments"]
            }
        }
    })
}

#[derive(Deserialize)]
struct CleanupPlan {
    segments: Vec<CleanupSegmentPlan>,
}

#[derive(Deserialize)]
struct CleanupSegmentPlan {
    start_index: usize,
    end_index: usize,
    text: String,
    #[serde(default)]
    speaker: Option<String>,
}

fn extract_json_object(raw_response: &str) -> Result<String> {
    if let (Some(start), Some(end)) = (raw_response.find('{'), raw_response.rfind('}')) {
        return Ok(raw_response[start..=end].to_string());
    }

    Err(anyhow::anyhow!(
        "Failed to find JSON object in cleanup response"
    ))
}

fn apply_cleanup_plan(
    transcript: &[TranscriptSegment],
    cleanup_plan: CleanupPlan,
) -> Result<Vec<TranscriptSegment>> {
    if transcript.is_empty() {
        return Ok(Vec::new());
    }

    if cleanup_plan.segments.is_empty() {
        return Err(anyhow::anyhow!("Cleanup plan returned no segments"));
    }

    let mut expected_start = 0usize;
    let mut cleaned_segments = Vec::with_capacity(cleanup_plan.segments.len());

    for cleaned in cleanup_plan.segments {
        if cleaned.start_index != expected_start {
            return Err(anyhow::anyhow!(
                "Cleanup plan is not contiguous at segment index {}",
                expected_start
            ));
        }

        if cleaned.end_index < cleaned.start_index || cleaned.end_index >= transcript.len() {
            return Err(anyhow::anyhow!(
                "Cleanup plan range {}-{} is out of bounds",
                cleaned.start_index,
                cleaned.end_index
            ));
        }

        if cleaned.text.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "Cleanup plan returned empty text for range {}-{}",
                cleaned.start_index,
                cleaned.end_index
            ));
        }

        let source_segments = &transcript[cleaned.start_index..=cleaned.end_index];
        let first = source_segments
            .first()
            .ok_or_else(|| anyhow::anyhow!("Cleanup plan referenced an empty range"))?;
        let last = source_segments.last().unwrap();

        let distinct_speakers = source_segments
            .iter()
            .map(|segment| segment.speaker.as_str())
            .collect::<std::collections::HashSet<_>>();

        if distinct_speakers.len() > 1 {
            return Err(anyhow::anyhow!(
                "Cleanup plan merged segments with different speakers between {} and {}",
                cleaned.start_index,
                cleaned.end_index
            ));
        }

        let merged_words = source_segments
            .iter()
            .filter_map(|segment| segment.words.as_ref())
            .flat_map(|words| words.iter().cloned())
            .collect::<Vec<_>>();

        cleaned_segments.push(TranscriptSegment {
            start: first.start.clone(),
            end: last.end.clone(),
            speaker: cleaned
                .speaker
                .as_ref()
                .map(|speaker| speaker.trim())
                .filter(|speaker| !speaker.is_empty())
                .unwrap_or(first.speaker.as_str())
                .to_string(),
            text: cleaned.text.trim().to_string(),
            words: (!merged_words.is_empty()).then_some(merged_words),
            alternatives: None,
            merge_status: None,
            active_source: None,
            similarity_score: None,
        });

        expected_start = cleaned.end_index + 1;
    }

    if expected_start != transcript.len() {
        return Err(anyhow::anyhow!(
            "Cleanup plan ended at {}, but transcript has {} segments",
            expected_start,
            transcript.len()
        ));
    }

    Ok(cleaned_segments)
}

#[derive(Clone)]
pub struct GeminiClient {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

/// Translation requests in flight at once.
const MAX_PARALLEL_TRANSLATIONS: usize = 4;

impl GeminiClient {
    pub fn new(api_key: String, base_url: String, model: String) -> Self {
        Self {
            client: crate::http::http_client(),
            api_key,
            base_url,
            model,
        }
    }

    pub async fn translate_transcript(
        &self,
        transcript: Vec<TranscriptSegment>,
        target_language: String,
        context: String,
    ) -> Result<String> {
        info!(
            "Starting translation of {} segments to {}",
            transcript.len(),
            target_language
        );
        let chunk_size = 20;
        let chunks: Vec<Vec<TranscriptSegment>> =
            transcript.chunks(chunk_size).map(|c| c.to_vec()).collect();

        let mut handles = vec![];
        // A long transcript has hundreds of chunks; firing them all at once
        // trips provider rate limits.
        let in_flight = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_TRANSLATIONS));

        for (i, chunk) in chunks.into_iter().enumerate() {
            let client = self.clone();
            let target_language = target_language.clone();
            let context = context.clone();
            let in_flight = in_flight.clone();

            handles.push(tokio::spawn(async move {
                let _permit = in_flight.acquire_owned().await?;
                match client
                    .translate_chunk(chunk, target_language, context, i)
                    .await
                {
                    Ok(res) => Ok(res),
                    Err(e) => {
                        error!("Translation chunk #{} failed: {}", i, e);
                        Err(e)
                    }
                }
            }));
        }

        let mut all_segments = vec![];
        // Await in order to preserve order
        for handle in handles {
            let res_str = handle.await??;

            // Clean up markdown code blocks if present
            let json_str = if let Some(start) = res_str.find('[') {
                if let Some(end) = res_str.rfind(']') {
                    &res_str[start..=end]
                } else {
                    &res_str
                }
            } else {
                &res_str
            };

            let segments: Vec<TranscriptSegment> = serde_json::from_str(json_str)?;
            all_segments.extend(segments);
        }

        Ok(serde_json::to_string(&all_segments)?)
    }

    async fn translate_chunk(
        &self,
        chunk: Vec<TranscriptSegment>,
        target_language: String,
        context: String,
        chunk_index: usize,
    ) -> Result<String> {
        debug!(
            "Translating chunk #{} ({} segments)",
            chunk_index,
            chunk.len()
        );
        let transcript_json = serde_json::to_string(&chunk)?;

        let system_prompt = "You are a professional translator. Your task is to translate the text content of a transcript while preserving the structure and timestamps exactly.";
        let user_prompt = format!(
            "Translate the 'text' field of the following JSON transcript segments into {}.
            
            Context:
            {}
            
            Constraints:
            - Preserve 'start', 'end', and 'speaker' fields exactly.
            - Only translate the 'text' field.
            - Return a strict JSON array of objects.
            - Do not translate speaker names.
            - This is chunk #{} of the transcript.
            - IMPORTANT: Keep translated text concise (max 84 characters per segment) for comfortable subtitle display
            - If translation is longer than original, consider breaking into natural shorter segments

            Example Input:
            [{{\"start\": \"00:00\", \"end\": \"00:05\", \"speaker\": \"Speaker 1\", \"text\": \"Hello world\"}}]

            Example Output (if target is Spanish):
            [{{\"start\": \"00:00\", \"end\": \"00:05\", \"speaker\": \"Speaker 1\", \"text\": \"Hola mundo\"}}]
            
            Transcript:
            {}",
            target_language, context, chunk_index + 1, transcript_json
        );

        self.send_request(system_prompt, &user_prompt).await
    }

    #[allow(clippy::too_many_arguments)] // one per request option; see the analyze_audio command
    pub async fn analyze_audio(
        &self,
        context: &str,
        glossary: &str,
        speaker_count: Option<u32>,
        remove_filler_words: bool,
        enforce_json_schema: bool,
        audio_uri: Option<&str>,
        audio_base64: Option<&str>,
    ) -> Result<String> {
        let mut system_prompt = "You are a professional video editor assistant. Your task is to transcribe the audio and identify logical segments.".to_string();

        if let Some(count) = speaker_count {
            system_prompt.push_str(&format!(" There are {} speakers in this audio. Use stable labels for exactly those speakers.", count));
        }

        let mut user_prompt = format!(
            "Analyze the following audio.\nContext: {}\nGlossary: {}\n[WISH FOR TIMESTAMPS]: Please output the transcription in a strict JSON format with 'start', 'end', 'speaker', and 'text' fields. Ensure timestamps are in 'MM:SS' format.\n",
            context, glossary
        );

        user_prompt.push_str(&format!("Example Output: {}\n", OutputFormat::example()));

        user_prompt.push_str(
            r#"
SUBTITLE LENGTH GUIDELINES (IMPORTANT):
- Each text segment should be concise and readable as subtitles
- Aim for maximum 84 characters per segment (42 chars per line × 2 lines)
- Break long sentences into multiple segments at natural pauses
- Each segment should be comfortable to read within its duration
- Avoid wall-of-text segments that would be difficult to read quickly
- Natural breaks: end segments at sentence boundaries, clause breaks, or natural speech pauses
"#,
        );

        user_prompt.push_str(
            r#"
GLOSSARY AND CONTEXT GUIDANCE (IMPORTANT):
- Treat glossary entries as authoritative spellings for names, products, companies, acronyms, and technical terms.
- Use the context to disambiguate phonetically similar words and to infer the correct domain-specific terminology.
- If the audio sounds like a glossary term or a contextually obvious proper noun, prefer the glossary/context spelling over a literal phonetic guess.
- Do not leave obvious ASR misspellings for technical words, names, or branded terms when the glossary or context makes the intended wording clear.
- Preserve casing for acronyms, product names, and proper nouns when context or glossary indicates it.
"#,
        );

        user_prompt.push_str(
            r#"
SPEAKER LABEL GUIDANCE (IMPORTANT):
- Keep speaker labels stable across the whole transcript.
- If a speaker's real name is clearly spoken in the audio, use that name as the speaker label.
- You may also use a real name when it is strongly inferable from the provided context or glossary.
- Only name a speaker when the evidence is clear. If there is any real uncertainty, use neutral labels such as Speaker 1, Speaker 2, etc.
- Do not guess names just because a name appears somewhere in the context.
- Prefer concise speaker labels like "Alice", "Dr. Weber", or "Moderator" over long descriptions.
"#,
        );

        if remove_filler_words {
            user_prompt.push_str("IMPORTANT: Remove only obvious filler words and short disfluencies such as 'um', 'uh', 'like', 'you know', brief false starts, and immediate word repetitions. Do not rewrite, paraphrase, summarize, or replace whole sentences. Preserve the original meaning, order, and wording except for the minimal filler/disfluency tokens you remove. Also remove non-voice sounds such as coughs or breaths when they appear in the text.\n");
        }

        // Determine if this is a Google API or OpenAI-compatible API
        let is_google_api = self.base_url.contains("generativelanguage.googleapis.com");

        let payload = if is_google_api {
            build_google_analyze_payload(&system_prompt, &user_prompt, audio_uri, audio_base64)
        } else {
            build_openai_analyze_payload(
                &self.model,
                &system_prompt,
                &user_prompt,
                audio_base64,
                enforce_json_schema,
            )
        };

        let base_url = self.base_url.trim_end_matches('/');
        let url = if is_google_api {
            // Google uses query parameter for API key
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                base_url, self.model, self.api_key
            )
        } else {
            // OpenAI/LiteLLM use path-based endpoint
            format!("{}/v1/chat/completions", base_url)
        };

        match self
            .execute_ai_json_request(
                &url,
                &payload,
                is_google_api,
                &RetryConfig::ai_request(false),
            )
            .await
        {
            Ok(text) => Ok(text),
            Err(err)
                if should_retry_analysis_without_schema(
                    &err,
                    is_google_api,
                    enforce_json_schema,
                ) =>
            {
                warn!(
                    "Structured transcript response failed on '{}' with '{}'. Retrying once without json_schema enforcement.",
                    redact_url(&url),
                    err
                );

                let fallback_payload = build_openai_analyze_payload(
                    &self.model,
                    &system_prompt,
                    &user_prompt,
                    audio_base64,
                    false,
                );

                self.execute_ai_json_request(
                    &url,
                    &fallback_payload,
                    is_google_api,
                    &RetryConfig::ai_request(false),
                )
                .await
                    .map_err(|fallback_err| {
                        anyhow::anyhow!(
                            "Structured transcript request failed and fallback without json_schema also failed: {}",
                            fallback_err
                        )
                    })
            }
            Err(err) => Err(anyhow::anyhow!("{}", err)),
        }
    }

    pub async fn cleanup_local_transcript(
        &self,
        transcript: Vec<TranscriptSegment>,
        context: &str,
        glossary: &str,
        remove_filler_words: bool,
    ) -> Result<Vec<TranscriptSegment>> {
        let indexed_transcript = transcript
            .iter()
            .enumerate()
            .map(|(index, segment)| {
                json!({
                    "index": index,
                    "start": segment.start,
                    "end": segment.end,
                    "speaker": segment.speaker,
                    "text": segment.text,
                })
            })
            .collect::<Vec<_>>();

        let transcript_json = serde_json::to_string(&indexed_transcript)?;
        let system_prompt = "You are a transcript cleanup editor. You clean ASR output while preserving timing coverage exactly.";
        let mut user_prompt = format!(
            "Clean this transcript that was produced by a local ASR model.

Context:
{}

Glossary:
{}

Rules:
- Preserve the original meaning and chronology.
- You MAY merge adjacent segments, but only if they are contiguous and from the same speaker.
- Do NOT reorder content.
- Do NOT skip content or leave timing gaps.
- Do NOT invent timestamps.
- Return JSON with a 'segments' array.
- Each output item must contain:
  - 'start_index': index of the first source segment included
  - 'end_index': index of the last source segment included
  - 'text': the cleaned text for that contiguous range
- The output ranges must cover every input segment exactly once, in order, with no overlaps.
- Keep timing stable by using only contiguous source ranges.
- Improve punctuation, capitalization, spelling, and obvious ASR mistakes.
- Treat glossary entries as authoritative spellings for names, products, companies, acronyms, and technical terms.
- Use the context to resolve special vocabulary, domain terms, personal names, and branded words that may be misspelled phonetically in the ASR output.
- When glossary and context make the intended wording clear, replace phonetic or malformed spellings with the canonical form.
- Prefer technically precise wording from the glossary/context over generic or phonetically similar alternatives.
- Preserve casing for acronyms, proper nouns, and product names.
- You MAY replace a generic speaker label with a real name only when that name is clearly spoken in the transcript or strongly inferable from context/glossary.
- If the speaker identity is uncertain, keep the existing generic speaker label.
- Keep speaker labels stable and consistent across all segments belonging to the same person.
- If you return a renamed speaker, include a 'speaker' field for that output item. Otherwise omit it.
",
            context, glossary
        );

        if remove_filler_words {
            user_prompt.push_str(
                "- Remove only obvious filler words, short disfluencies, and immediate repetitions when that improves readability.\n- Do not paraphrase, summarize, or rewrite complete sentences.\n- Keep the original wording and sentence structure except for the minimal filler/disfluency tokens you remove.\n",
            );
        }

        user_prompt.push_str(&format!("\nTranscript:\n{}", transcript_json));

        let is_google_api = self.base_url.contains("generativelanguage.googleapis.com");
        let payload = if is_google_api {
            json!({
                "contents": [{
                    "role": "user",
                    "parts": [{ "text": user_prompt }]
                }],
                "system_instruction": {
                    "parts": [{ "text": system_prompt }]
                },
                "generationConfig": {
                    "responseMimeType": "application/json"
                }
            })
        } else {
            json!({
                "model": self.model,
                "messages": [
                    {
                        "role": "system",
                        "content": system_prompt
                    },
                    {
                        "role": "user",
                        "content": user_prompt
                    }
                ],
                "response_format": cleanup_response_format()
            })
        };

        let base_url = self.base_url.trim_end_matches('/');
        let url = if is_google_api {
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                base_url, self.model, self.api_key
            )
        } else {
            format!("{}/v1/chat/completions", base_url)
        };

        let response = self
            .execute_ai_json_request(
                &url,
                &payload,
                is_google_api,
                &RetryConfig::ai_request(true),
            )
            .await
            .map_err(|error| anyhow::anyhow!("{}", error))?;

        let cleanup_json = match extract_json_object(&response) {
            Ok(cleanup_json) => cleanup_json,
            Err(error) => {
                warn!(
                    "Cleanup response could not be parsed as JSON object: {}. Falling back to original transcript. Response preview: {}",
                    error,
                    preview_for_error(&response)
                );
                return Ok(transcript);
            }
        };

        let cleanup_plan: CleanupPlan = match serde_json::from_str(&cleanup_json) {
            Ok(cleanup_plan) => cleanup_plan,
            Err(error) => {
                warn!(
                    "Cleanup response JSON schema parse failed: {}. Falling back to original transcript. Response preview: {}",
                    error,
                    preview_for_error(&cleanup_json)
                );
                return Ok(transcript);
            }
        };

        match apply_cleanup_plan(&transcript, cleanup_plan) {
            Ok(cleaned) => Ok(cleaned),
            Err(error) => {
                warn!(
                    "Cleanup plan validation failed: {}. Falling back to original transcript. Response preview: {}",
                    error,
                    preview_for_error(&cleanup_json)
                );
                Ok(transcript)
            }
        }
    }

    pub async fn merge_transcript_hypotheses(
        &self,
        primary_transcript: Vec<TranscriptSegment>,
        reference_transcript: Vec<TranscriptSegment>,
        context: &str,
        glossary: &str,
        remove_filler_words: bool,
    ) -> Result<Vec<TranscriptSegment>> {
        let primary_json = serde_json::to_string(
            &primary_transcript
                .iter()
                .enumerate()
                .map(|(index, segment)| {
                    json!({
                        "index": index,
                        "start": segment.start,
                        "end": segment.end,
                        "speaker": segment.speaker,
                        "text": segment.text,
                    })
                })
                .collect::<Vec<_>>(),
        )?;

        let reference_json = serde_json::to_string(&reference_transcript)?;
        let system_prompt = "You are a transcript reconciliation editor. You combine two transcript hypotheses while preserving the timing scaffold of the primary transcript exactly.";
        let mut user_prompt = format!(
            "Merge these two transcript hypotheses.

PRIMARY TRANSCRIPT:
- This transcript provides the timing scaffold, segment order, and speaker segmentation to preserve.
- Your output MUST map only onto contiguous ranges of the primary transcript.

REFERENCE TRANSCRIPT:
- Use this as an alternative hypothesis for wording, spelling, names, acronyms, and technical terminology.
- The reference transcript may be more accurate for some words, but its timestamps and segment boundaries must NOT be used directly.

Context:
{}

Glossary:
{}

Rules:
- Preserve the original meaning and chronology.
- Prefer the primary transcript's timing and speaker boundaries.
- Use the reference transcript, glossary, and context to improve spelling, names, branded terms, acronyms, and technical vocabulary.
- When the reference transcript clearly captures wording better than the primary transcript, prefer that wording.
- You MAY merge adjacent primary segments, but only if they are contiguous and from the same speaker.
- Do NOT reorder content.
- Do NOT skip content or leave timing gaps.
- Do NOT invent timestamps.
- Return JSON with a 'segments' array.
- Each output item must contain:
  - 'start_index': index of the first primary segment included
  - 'end_index': index of the last primary segment included
  - 'text': the merged text for that contiguous primary range
- The output ranges must cover every primary segment exactly once, in order, with no overlaps.
- Treat glossary entries as authoritative spellings for names, products, companies, acronyms, and technical terms.
- Use the context to resolve special vocabulary, personal names, domain terms, and phonetically ambiguous words.
- Preserve casing for acronyms, proper nouns, and product names.
",
            context, glossary
        );

        if remove_filler_words {
            user_prompt.push_str(
                "- Remove only obvious filler words, short disfluencies, and immediate repetitions when that improves readability.\n- Do not paraphrase, summarize, or rewrite complete sentences.\n- Keep the original wording and sentence structure except for the minimal filler/disfluency tokens you remove.\n",
            );
        }

        user_prompt.push_str(&format!(
            "\nPrimary transcript:\n{}\n\nReference transcript:\n{}",
            primary_json, reference_json
        ));

        let is_google_api = self.base_url.contains("generativelanguage.googleapis.com");
        let payload = if is_google_api {
            json!({
                "contents": [{
                    "role": "user",
                    "parts": [{ "text": user_prompt }]
                }],
                "system_instruction": {
                    "parts": [{ "text": system_prompt }]
                },
                "generationConfig": {
                    "responseMimeType": "application/json"
                }
            })
        } else {
            json!({
                "model": self.model,
                "messages": [
                    {
                        "role": "system",
                        "content": system_prompt
                    },
                    {
                        "role": "user",
                        "content": user_prompt
                    }
                ],
                "response_format": cleanup_response_format()
            })
        };

        let base_url = self.base_url.trim_end_matches('/');
        let url = if is_google_api {
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                base_url, self.model, self.api_key
            )
        } else {
            format!("{}/v1/chat/completions", base_url)
        };

        let response = self
            .execute_ai_json_request(
                &url,
                &payload,
                is_google_api,
                &RetryConfig::ai_request(true),
            )
            .await
            .map_err(|error| anyhow::anyhow!("{}", error))?;

        let cleanup_json = match extract_json_object(&response) {
            Ok(cleanup_json) => cleanup_json,
            Err(error) => {
                warn!(
                    "Merged transcript response could not be parsed as JSON object: {}. Falling back to primary transcript. Response preview: {}",
                    error,
                    preview_for_error(&response)
                );
                return Ok(primary_transcript);
            }
        };

        let cleanup_plan: CleanupPlan = match serde_json::from_str(&cleanup_json) {
            Ok(cleanup_plan) => cleanup_plan,
            Err(error) => {
                warn!(
                    "Merged transcript response JSON schema parse failed: {}. Falling back to primary transcript. Response preview: {}",
                    error,
                    preview_for_error(&cleanup_json)
                );
                return Ok(primary_transcript);
            }
        };

        match apply_cleanup_plan(&primary_transcript, cleanup_plan) {
            Ok(merged) => Ok(merged),
            Err(error) => {
                warn!(
                    "Merged transcript plan validation failed: {}. Falling back to primary transcript. Response preview: {}",
                    error,
                    preview_for_error(&cleanup_json)
                );
                Ok(primary_transcript)
            }
        }
    }

    pub async fn generate_clips(
        &self,
        transcript: &str,
        count: u32,
        min_duration: u32,
        max_duration: u32,
        topic: Option<String>,
        splicing: bool,
    ) -> Result<String> {
        let system_prompt = "You are a viral content expert. Your goal is to identify the most engaging moments in a video transcript for social media clips (TikTok, Reels, Shorts).";

        let mut user_prompt = format!(
            "Analyze the following transcript and identify the top {} most interesting clips.
            Constraints:
            - Each clip must be between {} and {} seconds long.
            - Clips should be self-contained and engaging.
            ",
            count, min_duration, max_duration
        );

        if let Some(t) = topic {
            user_prompt.push_str(&format!("- Focus specifically on the topic: '{}'.\n", t));
        }

        if splicing {
            user_prompt.push_str("- You MAY combine multiple non-contiguous segments into a single clip if they form a coherent narrative. \n");
            user_prompt.push_str("- Return a strict JSON array of objects with fields: 'segments' (array of {start, end}), 'title' (catchy title), 'reason' (why this is good).\n");
        } else {
            user_prompt.push_str("- Return a strict JSON array of objects with fields: 'segments' (array with ONE {start, end} object), 'title' (catchy title), 'reason' (why this is good).\n");
        }

        user_prompt.push_str(&format!(
            "Transcript:
            {}",
            transcript
        ));

        // Determine if this is a Google API or OpenAI-compatible API
        let is_google_api = self.base_url.contains("generativelanguage.googleapis.com");

        let payload = if is_google_api {
            // Google format
            json!({
                "contents": [{
                    "role": "user",
                    "parts": [{ "text": user_prompt }]
                }],
                "system_instruction": {
                    "parts": [{ "text": system_prompt }]
                }
            })
        } else {
            // OpenAI format
            json!({
                "model": self.model,
                "messages": [
                    {
                        "role": "system",
                        "content": system_prompt
                    },
                    {
                        "role": "user",
                        "content": user_prompt
                    }
                ]
            })
        };

        let base_url = self.base_url.trim_end_matches('/');
        let url = if is_google_api {
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                base_url, self.model, self.api_key
            )
        } else {
            format!("{}/v1/chat/completions", base_url)
        };

        let mut request = self.client.post(&url).json(&payload);

        if !is_google_api {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let res_json = execute_json_request(request, &url)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        let text = extract_text_from_response(&res_json, is_google_api)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(text)
    }

    /// Generate a podcast script from transcript
    pub async fn generate_podcast(
        &self,
        transcript: &str,
        min_duration: u32,
        max_duration: u32,
        context: Option<String>,
    ) -> Result<String> {
        let system_prompt = r#"You are an expert podcast editor and producer. Your goal is to create cohesive, engaging podcast episodes from longer transcripts by selecting the most compelling segments that form a natural narrative flow. You excel at creating smooth transitions between topics and identifying where bridge content is needed."#;

        let context_str = context.unwrap_or_else(|| "General content".to_string());

        let user_prompt = format!(
            r#"Create a podcast episode from the following transcript.

**Target Duration**: Between {} and {} seconds (approximately {}-{} minutes)

**Context**: {}

**CRITICAL Requirements**:

1. **Narrative Flow & Transitions**:
   - Select segments that naturally flow into each other
   - When transitioning between different topics, try to find segments that bridge the ideas
   - Look for moments where speakers reference previous or upcoming topics
   - Prioritize segments that end with natural transition points

2. **Voiceover Segments for Difficult Transitions**:
   - When you CANNOT find a natural transition between two topics, insert a "voiceover" segment
   - Voiceover segments suggest what a narrator/host could say to bridge the topics
   - These are placeholders for future TTS or user recording
   - Voiceover text should be concise (1-2 sentences) and professional

3. **Content Quality**:
   - Ensure an engaging opening that hooks listeners
   - Remove tangents or redundant content
   - Include a satisfying conclusion
   - Preserve speaker attributions

**Output Format**: Return a strict JSON object with this structure:
{{
  "title": "Engaging episode title",
  "summary": "2-3 sentence episode summary",
  "segments": [
    {{
      "start": "MM:SS",
      "end": "MM:SS",
      "text": "The actual transcript text",
      "speaker": "Speaker name",
      "segment_type": "content",
      "include_reason": "Brief reason why this segment is included"
    }},
    {{
      "start": "00:00",
      "end": "00:00",
      "text": "",
      "speaker": "Narrator",
      "segment_type": "voiceover",
      "transition_note": "Suggested voiceover text to bridge to the next topic, e.g. 'Speaking of technology, let's hear what our guest thinks about AI...'"
    }}
  ]
}}

**Segment Types**:
- "content": Actual audio from the source that will be exported
- "voiceover": Placeholder for narrator bridge (start/end can be "00:00", text can be empty)

**Transcript**:
{}"#,
            min_duration,
            max_duration,
            min_duration / 60,
            max_duration / 60,
            context_str,
            transcript
        );

        self.send_request(system_prompt, &user_prompt).await
    }

    /// Refine podcast script when duration doesn't fit constraints
    pub async fn refine_podcast(
        &self,
        original_transcript: &str,
        current_script: &str,
        current_duration: f64,
        target_min: u32,
        target_max: u32,
    ) -> Result<String> {
        let system_prompt = r#"You are an expert podcast editor and producer. You need to adjust an existing podcast script to fit within duration constraints while maintaining narrative coherence and smooth transitions."#;

        let direction = if current_duration < target_min as f64 {
            format!(
                "LENGTHEN the episode by adding {} more seconds of content",
                target_min as f64 - current_duration
            )
        } else {
            format!(
                "SHORTEN the episode by removing {} seconds of content",
                current_duration - target_max as f64
            )
        };

        let user_prompt = format!(
            r#"Adjust this podcast script to fit the target duration.

**Current Duration**: {:.0} seconds (content segments only, voiceover segments don't count)
**Target Range**: {}-{} seconds
**Required Action**: {}

**Current Script**:
{}

**Original Transcript** (for reference when adding content):
{}

**Requirements**:
1. Maintain narrative coherence and flow
2. Keep the engaging opening
3. Ensure a satisfying conclusion
4. Preserve or improve transitions between topics
5. When adding/removing content creates awkward transitions, add/update voiceover segments
6. Voiceover segments (segment_type: "voiceover") don't count toward duration

**Output Format**: Return the same JSON structure with segment_type for each segment:
{{
  "title": "Episode title",
  "summary": "Episode summary",
  "segments": [
    {{
      "start": "MM:SS",
      "end": "MM:SS",
      "text": "Content text",
      "speaker": "Speaker name",
      "segment_type": "content",
      "include_reason": "Reason"
    }},
    {{
      "start": "00:00",
      "end": "00:00",
      "text": "",
      "speaker": "Narrator",
      "segment_type": "voiceover",
      "transition_note": "Bridge text for narrator"
    }}
  ]
}}"#,
            current_duration,
            target_min,
            target_max,
            direction,
            current_script,
            original_transcript
        );

        self.send_request(system_prompt, &user_prompt).await
    }

    /// Helper to send a request to the API with retry logic
    async fn send_request(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let is_google_api = self.base_url.contains("generativelanguage.googleapis.com");
        let base_url = self.base_url.trim_end_matches('/').to_string();
        let model = self.model.clone();
        let api_key = self.api_key.clone();

        let payload = if is_google_api {
            json!({
                "contents": [{
                    "role": "user",
                    "parts": [{ "text": user_prompt }]
                }],
                "system_instruction": {
                    "parts": [{ "text": system_prompt }]
                },
                "generationConfig": {
                    "responseMimeType": "application/json"
                }
            })
        } else {
            json!({
                "model": model,
                "messages": [
                    {
                        "role": "system",
                        "content": system_prompt
                    },
                    {
                        "role": "user",
                        "content": user_prompt
                    }
                ],
                "response_format": { "type": "json_object" }
            })
        };

        let url = if is_google_api {
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                base_url, model, api_key
            )
        } else {
            format!("{}/v1/chat/completions", base_url)
        };

        let client = self.client.clone();
        let retry_config = RetryConfig::default();

        let result = retry_with_backoff(
            || async {
                let mut request = client.post(&url).json(&payload);

                if !is_google_api {
                    request = request.header("Authorization", format!("Bearer {}", api_key));
                }

                let res_json = execute_json_request(request, &url).await?;
                let text = extract_text_from_response(&res_json, is_google_api)?;

                Ok::<String, RetryableError>(text)
            },
            &retry_config,
            "API request",
        )
        .await;

        match result {
            Ok(text) => Ok(text),
            Err(retry_err) => Err(anyhow::anyhow!("{}", retry_err)),
        }
    }

    /// Ask for a JSON response that must match `schema` (a JSON Schema).
    /// OpenAI-compatible APIs get it as a strict `json_schema` response format;
    /// Gemini as a `responseSchema` (its OpenAPI subset, see
    /// [`to_gemini_schema`]). Returns the response text.
    pub(crate) async fn request_structured(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        schema_name: &str,
        schema: &Value,
    ) -> Result<String> {
        let is_google_api = self.base_url.contains("generativelanguage.googleapis.com");
        let base_url = self.base_url.trim_end_matches('/');
        let (url, payload) = if is_google_api {
            (
                format!(
                    "{}/v1beta/models/{}:generateContent?key={}",
                    base_url, self.model, self.api_key
                ),
                json!({
                    "contents": [{ "role": "user", "parts": [{ "text": user_prompt }] }],
                    "system_instruction": { "parts": [{ "text": system_prompt }] },
                    "generationConfig": {
                        "responseMimeType": "application/json",
                        "responseSchema": to_gemini_schema(schema)
                    }
                }),
            )
        } else {
            (
                format!("{}/v1/chat/completions", base_url),
                json!({
                    "model": self.model,
                    "messages": [
                        { "role": "system", "content": system_prompt },
                        { "role": "user", "content": user_prompt }
                    ],
                    "response_format": {
                        "type": "json_schema",
                        "json_schema": { "name": schema_name, "strict": true, "schema": schema }
                    }
                }),
            )
        };

        self.execute_ai_json_request(
            &url,
            &payload,
            is_google_api,
            &RetryConfig::ai_request(true),
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))
    }

    /// Send an AI request, retrying transient failures per `retry`.
    async fn execute_ai_json_request(
        &self,
        url: &str,
        payload: &Value,
        is_google_api: bool,
        retry: &RetryConfig,
    ) -> std::result::Result<String, RetryableError> {
        retry_with_backoff(
            || async {
                let mut request = self.client.post(url).json(payload);
                if !is_google_api {
                    request = request.header("Authorization", format!("Bearer {}", self.api_key));
                }
                let res_json = execute_json_request(request, url).await?;
                extract_text_from_response(&res_json, is_google_api)
            },
            retry,
            "AI request",
        )
        .await
    }
}

fn build_google_analyze_payload(
    system_prompt: &str,
    user_prompt: &str,
    audio_uri: Option<&str>,
    audio_base64: Option<&str>,
) -> Value {
    let mut contents = vec![json!({
        "role": "user",
        "parts": [{ "text": user_prompt }]
    })];

    if let Some(uri) = audio_uri {
        contents[0]["parts"].as_array_mut().unwrap().push(json!({
            "file_data": {
                "mime_type": "audio/ogg",
                "file_uri": uri
            }
        }));
    } else if let Some(base64) = audio_base64 {
        contents[0]["parts"].as_array_mut().unwrap().push(json!({
            "inline_data": {
                "mime_type": "audio/ogg",
                "data": base64
            }
        }));
    }

    json!({
        "contents": contents,
        "system_instruction": {
            "parts": [{ "text": system_prompt }]
        }
    })
}

fn build_openai_analyze_payload(
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    audio_base64: Option<&str>,
    enforce_json_schema: bool,
) -> Value {
    let mut user_content = vec![json!({
        "type": "text",
        "text": user_prompt
    })];

    if let Some(base64) = audio_base64 {
        user_content.push(json!({
            "type": "input_audio",
            "input_audio": {
                "data": base64,
                "format": "ogg"
            }
        }));
    }

    let mut payload = json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": system_prompt
            },
            {
                "role": "user",
                "content": user_content
            }
        ]
    });

    if enforce_json_schema {
        payload["response_format"] = transcript_response_format();
    }

    payload
}

fn should_retry_analysis_without_schema(
    err: &RetryableError,
    is_google_api: bool,
    enforce_json_schema: bool,
) -> bool {
    enforce_json_schema
        && !is_google_api
        && matches!(
            err,
            RetryableError::Server(message)
                if message
                    .to_ascii_lowercase()
                    .contains("error decoding response body")
        )
}

/// Redacts the value of a `key=` query parameter in a URL so API keys never
/// reach logs or error messages (logs are shipped to support via `zip_logs`).
fn redact_url(url: &str) -> String {
    let mut result = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(idx) = rest.find("key=") {
        // Only treat as a query parameter when preceded by `?` or `&`.
        let is_param = idx == 0 || matches!(rest.as_bytes().get(idx - 1), Some(b'?') | Some(b'&'));
        result.push_str(&rest[..idx + 4]);
        rest = &rest[idx + 4..];
        if is_param {
            let end = rest.find('&').unwrap_or(rest.len());
            result.push_str("[REDACTED]");
            rest = &rest[end..];
        }
    }
    result.push_str(rest);
    result
}

fn build_json_request(request: RequestBuilder) -> RequestBuilder {
    request
        .header(ACCEPT, "application/json")
        // Compression issues on some OpenAI-compatible gateways surface as
        // "error decoding response body". Prefer plain responses when possible.
        .header(ACCEPT_ENCODING, "identity")
}

async fn execute_json_request(
    request: RequestBuilder,
    url: &str,
) -> std::result::Result<Value, RetryableError> {
    let response = build_json_request(request)
        .send()
        .await
        .map_err(RetryableError::from)?;

    let status = response.status();
    let headers = format!("{:?}", response.headers());
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let safe_url = redact_url(url);

    debug!("AI response status from '{}': {}", safe_url, status);
    debug!("AI response headers from '{}': {}", safe_url, headers);

    let body = response.bytes().await.map_err(|err| {
        error!(
            "Failed to read AI response body from '{}': {}. Headers: {}",
            safe_url, err, headers
        );
        RetryableError::from(err)
    })?;

    let raw_body = String::from_utf8_lossy(&body).into_owned();
    debug!("Raw AI response body from '{}': {}", safe_url, raw_body);

    if status.as_u16() == 429 {
        warn!(
            "AI request to '{}' was rate limited: {}",
            safe_url, raw_body
        );
        return Err(RetryableError::RateLimited {
            retry_after: rate_limit_wait(retry_after.as_deref(), &raw_body),
        });
    }

    if !status.is_success() {
        error!(
            "AI request to '{}' failed with status {}. Raw response body: {}",
            safe_url, status, raw_body
        );
        return Err(RetryableError::Http {
            status: status.as_u16(),
            message: raw_body,
        });
    }

    serde_json::from_slice(&body).map_err(|err| {
        let preview = preview_for_error(&raw_body);
        error!(
            "Failed to parse AI response JSON from '{}': {}. Raw response body: {}",
            safe_url, err, raw_body
        );
        RetryableError::Permanent(format!(
            "Failed to parse response from '{}': {}. Raw body preview: {}",
            safe_url, err, preview
        ))
    })
}

fn extract_text_from_response(
    res_json: &Value,
    is_google_api: bool,
) -> std::result::Result<String, RetryableError> {
    if is_google_api {
        extract_google_text(res_json)
    } else {
        extract_openai_text(res_json)
    }
}

fn extract_google_text(res_json: &Value) -> std::result::Result<String, RetryableError> {
    let parts = res_json["candidates"][0]["content"]["parts"]
        .as_array()
        .ok_or_else(|| {
            RetryableError::Permanent(format!(
                "Missing Google response text parts. Raw JSON preview: {}",
                preview_for_error(&res_json.to_string())
            ))
        })?;

    let text = parts
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("");

    if text.is_empty() {
        return Err(RetryableError::Permanent(format!(
            "Google response contained no text parts. Raw JSON preview: {}",
            preview_for_error(&res_json.to_string())
        )));
    }

    Ok(text)
}

fn extract_openai_text(res_json: &Value) -> std::result::Result<String, RetryableError> {
    let message = &res_json["choices"][0]["message"]["content"];

    if let Some(text) = message.as_str() {
        return Ok(text.to_string());
    }

    if let Some(parts) = message.as_array() {
        let text = parts
            .iter()
            .filter_map(extract_openai_content_part_text)
            .collect::<Vec<_>>()
            .join("");

        if !text.is_empty() {
            return Ok(text);
        }
    }

    if let Some(text) = message["text"].as_str() {
        return Ok(text.to_string());
    }

    Err(RetryableError::Permanent(format!(
        "OpenAI-compatible response contained no readable message content. Raw JSON preview: {}",
        preview_for_error(&res_json.to_string())
    )))
}

fn extract_openai_content_part_text(part: &Value) -> Option<String> {
    if let Some(text) = part["text"].as_str() {
        return Some(text.to_string());
    }

    if let Some(text) = part["text"]["value"].as_str() {
        return Some(text.to_string());
    }

    None
}

fn preview_for_error(raw: &str) -> String {
    let total_chars = raw.chars().count();
    if total_chars <= RAW_RESPONSE_PREVIEW_LIMIT {
        return raw.to_string();
    }

    let preview: String = raw.chars().take(RAW_RESPONSE_PREVIEW_LIMIT).collect();
    format!("{}... [truncated, total_chars={}]", preview, total_chars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::TranscriptWord;

    #[test]
    fn apply_cleanup_plan_merges_contiguous_ranges() {
        let transcript = vec![
            TranscriptSegment {
                start: "00:00.000".into(),
                end: "00:01.000".into(),
                speaker: "Speaker 1".into(),
                text: "hello".into(),
                words: Some(vec![TranscriptWord {
                    start: "00:00.000".into(),
                    end: "00:01.000".into(),
                    text: "hello".into(),
                    speaker: Some("Speaker 1".into()),
                }]),
                alternatives: None,
                merge_status: None,
                active_source: None,
                similarity_score: None,
            },
            TranscriptSegment {
                start: "00:01.000".into(),
                end: "00:02.000".into(),
                speaker: "Speaker 1".into(),
                text: "world".into(),
                words: Some(vec![TranscriptWord {
                    start: "00:01.000".into(),
                    end: "00:02.000".into(),
                    text: "world".into(),
                    speaker: Some("Speaker 1".into()),
                }]),
                alternatives: None,
                merge_status: None,
                active_source: None,
                similarity_score: None,
            },
        ];

        let cleaned = apply_cleanup_plan(
            &transcript,
            CleanupPlan {
                segments: vec![CleanupSegmentPlan {
                    start_index: 0,
                    end_index: 1,
                    text: "Hello world.".into(),
                    speaker: None,
                }],
            },
        )
        .unwrap();

        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].start, "00:00.000");
        assert_eq!(cleaned[0].end, "00:02.000");
        assert_eq!(cleaned[0].text, "Hello world.");
        assert_eq!(cleaned[0].words.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn apply_cleanup_plan_can_rename_speaker_when_provided() {
        let transcript = vec![TranscriptSegment {
            start: "00:00.000".into(),
            end: "00:01.000".into(),
            speaker: "Speaker 1".into(),
            text: "hello".into(),
            words: None,
            alternatives: None,
            merge_status: None,
            active_source: None,
            similarity_score: None,
        }];

        let cleaned = apply_cleanup_plan(
            &transcript,
            CleanupPlan {
                segments: vec![CleanupSegmentPlan {
                    start_index: 0,
                    end_index: 0,
                    text: "Hello.".into(),
                    speaker: Some("Alice".into()),
                }],
            },
        )
        .unwrap();

        assert_eq!(cleaned[0].speaker, "Alice");
        assert_eq!(cleaned[0].text, "Hello.");
    }
}

/// Models an endpoint offers, for the model picker in Settings.
#[derive(Debug, PartialEq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelList {
    /// False when the endpoint has no model-listing route (404).
    pub supported: bool,
    pub models: Vec<String>,
}

/// List the models at `base_url` (Google: `/v1beta/models`; OpenAI-compatible:
/// `/v1/models`). The key goes in a header, never in the URL.
pub async fn list_models(client: &Client, base_url: &str, api_key: &str) -> Result<ModelList> {
    let base_url = base_url.trim_end_matches('/');
    let is_google_api = base_url.contains("generativelanguage.googleapis.com");
    let request = if is_google_api {
        client
            .get(format!("{base_url}/v1beta/models"))
            .header("x-goog-api-key", api_key)
    } else {
        client
            .get(format!("{base_url}/v1/models"))
            .bearer_auth(api_key)
    };

    let response = request
        .send()
        .await
        .with_context(|| format!("Failed to reach '{base_url}'"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(ModelList {
            supported: false,
            models: Vec::new(),
        });
    }
    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to fetch models: {}",
            response.status()
        ));
    }

    let body: Value = response
        .json()
        .await
        .context("Invalid model list response")?;
    let models: Vec<String> = if let Some(models) = body.get("models").and_then(Value::as_array) {
        models
            .iter()
            .filter_map(|model| model.get("name").and_then(Value::as_str))
            .map(|name| name.trim_start_matches("models/").to_string())
            .collect()
    } else if let Some(models) = body.get("data").and_then(Value::as_array) {
        models
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    } else {
        return Err(anyhow::anyhow!("Invalid model list response format"));
    };

    Ok(ModelList {
        supported: true,
        models,
    })
}

#[cfg(test)]
mod model_list_tests {
    use super::*;

    #[tokio::test]
    async fn lists_openai_compatible_models_with_a_bearer_key() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/models")
            .match_header("authorization", "Bearer sk-1")
            .with_body(r#"{"data":[{"id":"gpt-a"},{"id":"gpt-b"}]}"#)
            .create_async()
            .await;

        let list = list_models(&Client::new(), &format!("{}/", server.url()), "sk-1")
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(
            list,
            ModelList {
                supported: true,
                models: vec!["gpt-a".into(), "gpt-b".into()]
            }
        );
    }

    #[tokio::test]
    async fn endpoints_without_model_listing_are_reported_not_failed() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/models")
            .with_status(404)
            .create_async()
            .await;

        let list = list_models(&Client::new(), &server.url(), "sk-1")
            .await
            .unwrap();
        assert!(!list.supported);
    }
}

/// Convert a JSON Schema into the OpenAPI subset Gemini's `responseSchema`
/// accepts: upper-case type names, and only the keywords it knows.
pub(crate) fn to_gemini_schema(schema: &Value) -> Value {
    const KEPT: [&str; 9] = [
        "type",
        "description",
        "enum",
        "properties",
        "items",
        "required",
        "minimum",
        "maximum",
        "nullable",
    ];
    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if !KEPT.contains(&key.as_str()) {
                    continue;
                }
                let converted = match key.as_str() {
                    "type" => value
                        .as_str()
                        .map(|name| Value::String(name.to_ascii_uppercase()))
                        .unwrap_or_else(|| value.clone()),
                    "properties" => Value::Object(
                        value
                            .as_object()
                            .map(|props| {
                                props
                                    .iter()
                                    .map(|(name, prop)| (name.clone(), to_gemini_schema(prop)))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ),
                    "items" => to_gemini_schema(value),
                    _ => value.clone(),
                };
                out.insert(key.clone(), converted);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod gemini_schema_tests {
    use super::*;

    #[test]
    fn json_schema_becomes_geminis_openapi_subset() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "clips": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": { "score": { "type": "integer", "minimum": 1 } },
                        "required": ["score"]
                    }
                }
            },
            "required": ["clips"]
        });
        assert_eq!(
            to_gemini_schema(&schema),
            json!({
                "type": "OBJECT",
                "properties": {
                    "clips": {
                        "type": "ARRAY",
                        "items": {
                            "type": "OBJECT",
                            "properties": { "score": { "type": "INTEGER", "minimum": 1 } },
                            "required": ["score"]
                        }
                    }
                },
                "required": ["clips"]
            })
        );
    }
}
