use crate::video::TranscriptSegment;
use anyhow::Result;
use reqwest::Client;
use serde_json::{json, Value};

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
        let chunk_size = 20;
        let chunks: Vec<Vec<TranscriptSegment>> =
            transcript.chunks(chunk_size).map(|c| c.to_vec()).collect();

        let mut handles = vec![];

        for (i, chunk) in chunks.into_iter().enumerate() {
            let client = self.clone();
            let target_language = target_language.clone();
            let context = context.clone();

            handles.push(tokio::spawn(async move {
                client
                    .translate_chunk(chunk, target_language, context, i)
                    .await
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
        let transcript_json = serde_json::to_string(&chunk)?;

        let system_prompt = "You are a professional translator. Your task is to translate the text content of a transcript while preserving the structure and timestamps exactly.";
        let user_prompt = format!(
            "Translate the 'text' field of the following JSON transcript segments into {}.
            
            Context about the video: {}
            
            Constraints:
            - Preserve 'start', 'end', and 'speaker' fields exactly.
            - Only translate the 'text' field.
            - Return a strict JSON array of objects.
            - Do not translate speaker names.
            - This is chunk #{} of the transcript.

            Example Input:
            [{{\"start\": \"00:00\", \"end\": \"00:05\", \"speaker\": \"Speaker 1\", \"text\": \"Hello world\"}}]

            Example Output (if target is Spanish):
            [{{\"start\": \"00:00\", \"end\": \"00:05\", \"speaker\": \"Speaker 1\", \"text\": \"Hola mundo\"}}]
            
            Transcript:
            {}",
            target_language, context, chunk_index + 1, transcript_json
        );

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
                },
                "generationConfig": {
                    "responseMimeType": "application/json"
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
                ],
                "response_format": { "type": "json_object" }
            })
        };

        let url = if is_google_api {
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                self.base_url, self.model, self.api_key
            )
        } else {
            format!("{}/v1/chat/completions", self.base_url)
        };

        let mut request = self.client.post(&url).json(&payload);

        if !is_google_api {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("API failed: {}", response.text().await?));
        }

        let res_json: Value = response.json().await?;

        let text = if is_google_api {
            res_json["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .unwrap_or("No text response")
                .to_string()
        } else {
            res_json["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("No text response")
                .to_string()
        };

        Ok(text)
    }

    pub async fn analyze_audio(
        &self,
        context: &str,
        glossary: &str,
        speaker_count: Option<u32>,
        audio_uri: Option<&str>,
        audio_base64: Option<&str>,
    ) -> Result<String> {
        let mut system_prompt = "You are a professional video editor assistant. Your task is to transcribe the audio and identify logical segments.".to_string();

        if let Some(count) = speaker_count {
            system_prompt.push_str(&format!(" There are {} speakers in this audio. Please label them as Speaker 1, Speaker 2, etc.", count));
        }

        let user_prompt = format!(
            "Analyze the following audio.\nContext: {}\nGlossary: {}\n[WISH FOR TIMESTAMPS]: Please output the transcription in a strict JSON format with 'start', 'end', 'speaker', and 'text' fields. Ensure timestamps are in 'MM:SS' format.\n*Note: This prompt is exemplary; the model may hallucinate timestamp formats without few-shot examples. Please verify output.*",
            context, glossary
        );

        // Determine if this is a Google API or OpenAI-compatible API
        let is_google_api = self.base_url.contains("generativelanguage.googleapis.com");

        let payload = if is_google_api {
            // Google format
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
        } else {
            // OpenAI format
            // Some models support audio in messages, try to include it
            let mut user_content = vec![json!({
                "type": "text",
                "text": user_prompt
            })];

            // If we have base64 audio, include it
            if let Some(base64) = audio_base64 {
                user_content.push(json!({
                    "type": "input_audio",
                    "input_audio": {
                        "data": base64,
                        "format": "ogg"
                    }
                }));
            }

            json!({
                "model": self.model,
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
            })
        };

        let url = if is_google_api {
            // Google uses query parameter for API key
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                self.base_url, self.model, self.api_key
            )
        } else {
            // OpenAI/LiteLLM use path-based endpoint
            format!("{}/v1/chat/completions", self.base_url)
        };

        let mut request = self.client.post(&url).json(&payload);

        // Add Authorization header for non-Google APIs
        if !is_google_api {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("API failed: {}", response.text().await?));
        }

        let res_json: Value = response.json().await?;

        // Extract text from response (handle both Google and OpenAI formats)
        let text = if is_google_api {
            res_json["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .unwrap_or("No text response")
                .to_string()
        } else {
            // OpenAI format
            res_json["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("No text response")
                .to_string()
        };

        Ok(text)
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

        let url = if is_google_api {
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                self.base_url, self.model, self.api_key
            )
        } else {
            format!("{}/v1/chat/completions", self.base_url)
        };

        let mut request = self.client.post(&url).json(&payload);

        if !is_google_api {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("API failed: {}", response.text().await?));
        }

        let res_json: Value = response.json().await?;

        let text = if is_google_api {
            res_json["candidates"][0]["content"]["parts"][0]["text"]
                .as_str()
                .unwrap_or("No text response")
                .to_string()
        } else {
            res_json["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("No text response")
                .to_string()
        };

        Ok(text)
    }
}
