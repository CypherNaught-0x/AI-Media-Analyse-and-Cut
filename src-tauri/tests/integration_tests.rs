use ai_media_cutter_lib::gemini::GeminiClient;
use ai_media_cutter_lib::video::TranscriptSegment;
use mockito::Server;
use serde_json::json;
use std::env;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut header_end = None;
    let mut content_length = 0usize;

    loop {
        let bytes_read = stream.read(&mut chunk).await?;
        if bytes_read == 0 {
            break;
        }

        buffer.extend_from_slice(&chunk[..bytes_read]);

        if header_end.is_none() {
            if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                let end = pos + 4;
                header_end = Some(end);
                let headers = String::from_utf8_lossy(&buffer[..end]);
                content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
            }
        }

        if let Some(end) = header_end {
            if buffer.len() >= end + content_length {
                break;
            }
        }
    }

    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

async fn ensure_loopback_access(test_name: &str) -> bool {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            eprintln!(
                "Skipping {test_name}: loopback sockets are not permitted in this environment ({err})"
            );
            false
        }
        Err(err) => panic!("Failed to bind loopback socket for {test_name}: {err}"),
    }
}

#[tokio::test]
async fn test_transcription_mock() {
    if !ensure_loopback_access("test_transcription_mock").await {
        return;
    }

    let mut server = Server::new_async().await;
    // Since base_url is localhost, GeminiClient treats it as OpenAI-compatible
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "choices": [{
                    "message": {
                        "content": json!([
                            {
                                "start": "00:00",
                                "end": "00:05",
                                "speaker": "Speaker 1",
                                "text": "Hello world"
                            }
                        ]).to_string()
                    }
                }]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let result = client
        .analyze_audio("context", "glossary", None, false, true, None, None)
        .await
        .unwrap();

    // The result is a JSON string of segments. It might be wrapped in markdown code blocks.
    let json_str = if let Some(start) = result.find('[') {
        if let Some(end) = result.rfind(']') {
            &result[start..=end]
        } else {
            &result
        }
    } else {
        &result
    };

    let segments: Vec<TranscriptSegment> = serde_json::from_str(json_str).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "Hello world");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_transcription_mock_with_structured_content() {
    if !ensure_loopback_access("test_transcription_mock_with_structured_content").await {
        return;
    }

    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "choices": [{
                    "message": {
                        "content": [{
                            "type": "text",
                            "text": json!([
                                {
                                    "start": "00:00",
                                    "end": "00:05",
                                    "speaker": "Speaker 1",
                                    "text": "Hello from structured content"
                                }
                            ]).to_string()
                        }]
                    }
                }]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let result = client
        .analyze_audio("context", "glossary", None, false, true, None, None)
        .await
        .unwrap();

    let segments: Vec<TranscriptSegment> = serde_json::from_str(&result).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "Hello from structured content");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_transcription_invalid_json_body_includes_raw_preview() {
    if !ensure_loopback_access("test_transcription_invalid_json_body_includes_raw_preview").await {
        return;
    }

    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("not-json-response")
        .create_async()
        .await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let err = client
        .analyze_audio("context", "glossary", None, false, true, None, None)
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("Failed to parse response"));
    assert!(err.contains("Raw body preview: not-json-response"));

    mock.assert_async().await;
}

#[tokio::test]
async fn test_translation_mock() {
    if !ensure_loopback_access("test_translation_mock").await {
        return;
    }

    let mut server = Server::new_async().await;
    // Since base_url is localhost, GeminiClient treats it as OpenAI-compatible
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "choices": [{
                    "message": {
                        "content": json!([
                            {
                                "start": "00:00",
                                "end": "00:05",
                                "speaker": "Speaker 1",
                                "text": "Hola mundo"
                            }
                        ]).to_string()
                    }
                }]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let transcript = vec![TranscriptSegment {
        start: "00:00".to_string(),
        end: "00:05".to_string(),
        speaker: "Speaker 1".to_string(),
        text: "Hello world".to_string(),
        ..Default::default()
    }];

    let result = client
        .translate_transcript(transcript, "Spanish".to_string(), "context".to_string())
        .await
        .unwrap();

    let segments: Vec<TranscriptSegment> = serde_json::from_str(&result).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "Hola mundo");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_transcription_request_includes_json_schema_when_enabled() {
    if !ensure_loopback_access("test_transcription_request_includes_json_schema_when_enabled").await
    {
        return;
    }

    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_body(mockito::Matcher::PartialJsonString(
            r#"{"response_format":{"type":"json_schema","json_schema":{"name":"transcript_segments"}}}"#
                .to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "choices": [{
                    "message": {
                        "content": json!([
                            {
                                "start": "00:00",
                                "end": "00:05",
                                "speaker": "Speaker 1",
                                "text": "Hello world"
                            }
                        ]).to_string()
                    }
                }]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let result = client
        .analyze_audio("context", "glossary", None, false, true, None, None)
        .await
        .unwrap();

    let segments: Vec<TranscriptSegment> = serde_json::from_str(&result).unwrap();
    assert_eq!(segments.len(), 1);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_transcription_request_succeeds_when_json_schema_disabled() {
    if !ensure_loopback_access("test_transcription_request_succeeds_when_json_schema_disabled")
        .await
    {
        return;
    }

    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_body(mockito::Matcher::Regex(
            r#""model"\s*:\s*"gemini-1.5-flash""#.to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "choices": [{
                    "message": {
                        "content": json!([
                            {
                                "start": "00:00",
                                "end": "00:05",
                                "speaker": "Speaker 1",
                                "text": "Hello world"
                            }
                        ]).to_string()
                    }
                }]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let result = client
        .analyze_audio("context", "glossary", None, false, false, None, None)
        .await
        .unwrap();

    let segments: Vec<TranscriptSegment> = serde_json::from_str(&result).unwrap();
    assert_eq!(segments.len(), 1);

    mock.assert_async().await;
}

#[tokio::test]
async fn test_transcription_retries_without_json_schema_on_body_decode_error() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            eprintln!(
                "Skipping test_transcription_retries_without_json_schema_on_body_decode_error: loopback sockets are not permitted in this environment ({err})"
            );
            return;
        }
        Err(err) => panic!(
            "Failed to bind loopback socket for test_transcription_retries_without_json_schema_on_body_decode_error: {err}"
        ),
    };
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_for_server = Arc::clone(&requests);

    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await.unwrap();
            requests_for_server.lock().await.push(request);

            if attempt == 0 {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999\r\nConnection: close\r\n\r\n{\"choices\":[{\"message\":{\"content\":\"[]\"}}]}",
                    )
                    .await
                    .unwrap();
            } else {
                let body = json!({
                    "choices": [{
                        "message": {
                            "content": json!([
                                {
                                    "start": "00:00",
                                    "end": "00:05",
                                    "speaker": "Speaker 1",
                                    "text": "Fallback succeeded"
                                }
                            ]).to_string()
                        }
                    }]
                })
                .to_string();

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );

                stream.write_all(response.as_bytes()).await.unwrap();
            }
        }
    });

    let client = GeminiClient::new(
        "fake_key".to_string(),
        format!("http://{}", address),
        "gemini-1.5-flash".to_string(),
    );

    let result = client
        .analyze_audio("context", "glossary", None, false, true, None, None)
        .await
        .unwrap();

    let segments: Vec<TranscriptSegment> = serde_json::from_str(&result).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "Fallback succeeded");

    server.await.unwrap();

    let captured_requests = requests.lock().await;
    assert_eq!(captured_requests.len(), 2);
    assert!(captured_requests[0].contains("\"response_format\""));
    assert!(!captured_requests[1].contains("\"response_format\""));
}

#[tokio::test]
async fn test_generate_clips_mock() {
    if !ensure_loopback_access("test_generate_clips_mock").await {
        return;
    }

    let mut server = Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            json!({
                "choices": [{
                    "message": {
                        "content": json!([
                            {
                                "segments": [{"start": "00:00", "end": "00:10"}],
                                "title": "Viral Clip",
                                "reason": "Very funny"
                            }
                        ]).to_string()
                    }
                }]
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let result = client
        .generate_clips("transcript content", 1, 5, 60, None, false)
        .await
        .unwrap();

    let json_str = if let Some(start) = result.find('[') {
        if let Some(end) = result.rfind(']') {
            &result[start..=end]
        } else {
            &result
        }
    } else {
        &result
    };

    let clips: serde_json::Value = serde_json::from_str(json_str).unwrap();
    assert!(clips.is_array());
    assert_eq!(clips[0]["title"], "Viral Clip");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_real_pipeline() {
    let _ = dotenvy::dotenv();

    let api_key = env::var("TEST_API_KEY")
        .or_else(|_| env::var("API_KEY"))
        .unwrap_or_default();
    let base_url = env::var("TEST_BASE_URL")
        .or_else(|_| env::var("BASE_URL"))
        .unwrap_or_default();
    let model = env::var("TEST_MODEL")
        .or_else(|_| env::var("API_MODEL"))
        .unwrap_or_default();

    if api_key.is_empty() || base_url.is_empty() || model.is_empty() {
        println!("Skipping real pipeline test: API_KEY, BASE_URL, or API_MODEL not set or empty");
        return;
    }

    let client = GeminiClient::new(api_key, base_url, model);

    // Use the test file
    let mut test_file_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    test_file_path.push("../dev-resources/test-data/test_podcast.m4a");

    assert!(
        test_file_path.exists(),
        "Test file not found at {:?}",
        test_file_path
    );

    let audio_data = std::fs::read(&test_file_path).unwrap();
    use base64::{engine::general_purpose, Engine as _};
    let audio_base64 = general_purpose::STANDARD.encode(&audio_data);

    // 1. Transcription
    println!("Testing real transcription...");
    let result = client
        .analyze_audio(
            "This is a test podcast about AI.",
            "",
            None,
            false,
            true,
            None,
            Some(&audio_base64),
        )
        .await;
    assert!(result.is_ok(), "Transcription failed: {:?}", result.err());

    let transcript_json = result.unwrap();

    let json_str = if let Some(start) = transcript_json.find('[') {
        if let Some(end) = transcript_json.rfind(']') {
            &transcript_json[start..=end]
        } else {
            &transcript_json
        }
    } else {
        &transcript_json
    };

    let segments: Vec<TranscriptSegment> =
        serde_json::from_str(json_str).expect("Failed to parse JSON from response");

    assert!(!segments.is_empty(), "Transcript should not be empty");
    println!(
        "Transcription successful. Found {} segments.",
        segments.len()
    );

    // Load gold standard
    let mut gold_standard_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    gold_standard_path.push("../dev-resources/test-data/gold_standard_transcript.json");
    let gold_standard_str =
        std::fs::read_to_string(gold_standard_path).expect("Failed to read gold standard");
    let gold_standard: Vec<TranscriptSegment> =
        serde_json::from_str(&gold_standard_str).expect("Failed to parse gold standard");

    // Compare
    let actual_text: String = segments
        .iter()
        .map(|s| s.text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    let gold_text: String = gold_standard
        .iter()
        .map(|s| s.text.clone())
        .collect::<Vec<_>>()
        .join(" ");

    let similarity = calculate_similarity(&actual_text, &gold_text);
    println!("Transcript similarity: {:.2}", similarity);

    assert!(
        similarity > 0.8,
        "Transcript similarity too low: {:.2}",
        similarity
    );

    // 2. Translation
    println!("Testing real translation...");
    let translation_result = client
        .translate_transcript(
            segments.clone(),
            "German".to_string(),
            "Podcast context".to_string(),
        )
        .await;
    assert!(
        translation_result.is_ok(),
        "Translation failed: {:?}",
        translation_result.err()
    );

    let translated_json = translation_result.unwrap();
    let translated_segments: Vec<TranscriptSegment> =
        serde_json::from_str(&translated_json).expect("Failed to parse translated JSON");

    assert_eq!(
        translated_segments.len(),
        segments.len(),
        "Translation should have same number of segments"
    );

    println!("Translation successful.");

    // 3. Clip Generation
    println!("Testing real clip generation...");
    let transcript_text = serde_json::to_string(&segments).unwrap();
    let clips_result = client
        .generate_clips(&transcript_text, 1, 5, 60, Some("AI".to_string()), false)
        .await;
    assert!(
        clips_result.is_ok(),
        "Clip generation failed: {:?}",
        clips_result.err()
    );

    let clips_json = clips_result.unwrap();
    let clips_json_str = if let Some(start) = clips_json.find('[') {
        if let Some(end) = clips_json.rfind(']') {
            &clips_json[start..=end]
        } else {
            &clips_json
        }
    } else {
        &clips_json
    };

    let clips: serde_json::Value =
        serde_json::from_str(clips_json_str).expect("Failed to parse clips JSON");
    assert!(clips.is_array(), "Clips should be an array");
    assert!(
        !clips.as_array().unwrap().is_empty(),
        "Should generate at least one clip"
    );

    println!("Clip generation successful.");
}

fn calculate_similarity(s1: &str, s2: &str) -> f64 {
    let s1_words: std::collections::HashSet<_> =
        s1.split_whitespace().map(|s| s.to_lowercase()).collect();
    let s2_words: std::collections::HashSet<_> =
        s2.split_whitespace().map(|s| s.to_lowercase()).collect();

    let intersection = s1_words.intersection(&s2_words).count();
    let union = s1_words.union(&s2_words).count();

    if union == 0 {
        return 1.0;
    }

    intersection as f64 / union as f64
}
