use crate::retry::{retry_with_backoff, RetryConfig, RetryableError};
use crate::video::TranscriptSegment;
use anyhow::Result;
use log::{debug, error, info, warn};
use reqwest::{
    header::{ACCEPT, ACCEPT_ENCODING},
    Client, RequestBuilder,
};
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
                            "description": "Speaker label such as Speaker 1"
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

#[derive(Clone)]
pub struct GeminiClient {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl GeminiClient {
    pub fn new(api_key: String, base_url: String, model: String) -> Self {
        Self {
            client: Client::new(),
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

        for (i, chunk) in chunks.into_iter().enumerate() {
            let client = self.clone();
            let target_language = target_language.clone();
            let context = context.clone();

            handles.push(tokio::spawn(async move {
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
            system_prompt.push_str(&format!(" There are {} speakers in this audio. Please label them as Speaker 1, Speaker 2, etc.", count));
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

        if remove_filler_words {
            user_prompt.push_str("IMPORTANT: Remove all filler words (um, uh, like, you know) and non-voice sounds (coughs, breaths) from the 'text' field. The transcript should be clean and ready for subtitles.\n");
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
            .execute_ai_json_request(&url, &payload, is_google_api)
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
                    url, err
                );

                let fallback_payload = build_openai_analyze_payload(
                    &self.model,
                    &system_prompt,
                    &user_prompt,
                    audio_base64,
                    false,
                );

                self.execute_ai_json_request(&url, &fallback_payload, is_google_api)
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

        self.send_request(&system_prompt, &user_prompt).await
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

        self.send_request(&system_prompt, &user_prompt).await
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

    async fn execute_ai_json_request(
        &self,
        url: &str,
        payload: &Value,
        is_google_api: bool,
    ) -> std::result::Result<String, RetryableError> {
        let mut request = self.client.post(url).json(payload);

        if !is_google_api {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let res_json = execute_json_request(request, url).await?;
        extract_text_from_response(&res_json, is_google_api)
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

    debug!("AI response status from '{}': {}", url, status);
    debug!("AI response headers from '{}': {}", url, headers);

    let body = response.bytes().await.map_err(|err| {
        error!(
            "Failed to read AI response body from '{}': {}. Headers: {}",
            url, err, headers
        );
        RetryableError::from(err)
    })?;

    let raw_body = String::from_utf8_lossy(&body).into_owned();
    debug!("Raw AI response body from '{}': {}", url, raw_body);

    if !status.is_success() {
        error!(
            "AI request to '{}' failed with status {}. Raw response body: {}",
            url, status, raw_body
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
            url, err, raw_body
        );
        RetryableError::Permanent(format!(
            "Failed to parse response from '{}': {}. Raw body preview: {}",
            url, err, preview
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
