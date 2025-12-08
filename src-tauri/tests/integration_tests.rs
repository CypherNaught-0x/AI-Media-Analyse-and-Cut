use ai_media_cutter_lib::gemini::GeminiClient;
use ai_media_cutter_lib::video::TranscriptSegment;
use mockito::Server;
use serde_json::json;
use std::env;
use std::path::PathBuf;

#[tokio::test]
async fn test_transcription_mock() {
    let mut server = Server::new_async().await;
    // Since base_url is localhost, GeminiClient treats it as OpenAI-compatible
    let mock = server.mock("POST", "/v1/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({
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
        }).to_string())
        .create_async().await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let result = client.analyze_audio("context", "glossary", None, None, None).await.unwrap();
    
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
async fn test_translation_mock() {
    let mut server = Server::new_async().await;
    // Since base_url is localhost, GeminiClient treats it as OpenAI-compatible
    let mock = server.mock("POST", "/v1/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({
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
        }).to_string())
        .create_async().await;

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
    }];

    let result = client.translate_transcript(transcript, "Spanish".to_string(), "context".to_string()).await.unwrap();
    
    let segments: Vec<TranscriptSegment> = serde_json::from_str(&result).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "Hola mundo");

    mock.assert_async().await;
}

#[tokio::test]
async fn test_generate_clips_mock() {
    let mut server = Server::new_async().await;
    let mock = server.mock("POST", "/v1/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({
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
        }).to_string())
        .create_async().await;

    let client = GeminiClient::new(
        "fake_key".to_string(),
        server.url(),
        "gemini-1.5-flash".to_string(),
    );

    let result = client.generate_clips("transcript content", 1, 5, 60, None, false).await.unwrap();
    
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
    
    let api_key = env::var("TEST_API_KEY").or_else(|_| env::var("API_KEY")).ok();
    let base_url = env::var("TEST_BASE_URL").or_else(|_| env::var("BASE_URL")).ok();
    let model = env::var("TEST_MODEL").or_else(|_| env::var("API_MODEL")).ok();

    if api_key.is_none() || base_url.is_none() || model.is_none() {
        println!("Skipping real pipeline test: API_KEY, BASE_URL, or API_MODEL not set");
        return;
    }

    let client = GeminiClient::new(
        api_key.unwrap(),
        base_url.unwrap(),
        model.unwrap(),
    );

    // Use the test file
    let mut test_file_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    test_file_path.push("../dev-resources/test-data/test_podcast.m4a");
    
    assert!(test_file_path.exists(), "Test file not found at {:?}", test_file_path);

    let audio_data = std::fs::read(&test_file_path).unwrap();
    use base64::{Engine as _, engine::general_purpose};
    let audio_base64 = general_purpose::STANDARD.encode(&audio_data);

    // 1. Transcription
    println!("Testing real transcription...");
    let result = client.analyze_audio("This is a test podcast about AI.", "", None, None, Some(&audio_base64)).await;
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

    let segments: Vec<TranscriptSegment> = serde_json::from_str(json_str).expect("Failed to parse JSON from response");
    
    assert!(!segments.is_empty(), "Transcript should not be empty");
    println!("Transcription successful. Found {} segments.", segments.len());

    // 2. Translation
    println!("Testing real translation...");
    let translation_result = client.translate_transcript(segments.clone(), "German".to_string(), "Podcast context".to_string()).await;
    assert!(translation_result.is_ok(), "Translation failed: {:?}", translation_result.err());
    
    let translated_json = translation_result.unwrap();
    let translated_segments: Vec<TranscriptSegment> = serde_json::from_str(&translated_json).expect("Failed to parse translated JSON");
    
    assert_eq!(translated_segments.len(), segments.len(), "Translation should have same number of segments");
    
    println!("Translation successful.");

    // 3. Clip Generation
    println!("Testing real clip generation...");
    let transcript_text = serde_json::to_string(&segments).unwrap();
    let clips_result = client.generate_clips(&transcript_text, 1, 5, 60, Some("AI".to_string()), false).await;
    assert!(clips_result.is_ok(), "Clip generation failed: {:?}", clips_result.err());

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

    let clips: serde_json::Value = serde_json::from_str(clips_json_str).expect("Failed to parse clips JSON");
    assert!(clips.is_array(), "Clips should be an array");
    assert!(!clips.as_array().unwrap().is_empty(), "Should generate at least one clip");
    
    println!("Clip generation successful.");
}
