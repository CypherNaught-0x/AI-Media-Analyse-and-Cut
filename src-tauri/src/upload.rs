use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;
use std::path::Path;
use tokio::time::{sleep, Duration};

#[derive(Deserialize, Debug)]
struct FileResource {
    name: String,
    uri: String,
    state: String,
}

#[derive(Deserialize, Debug)]
struct UploadResponseCorrect {
    file: FileResource,
}

pub async fn upload_file_and_wait(
    api_key: &str,
    base_url: &str,
    path: &Path,
) -> Result<Option<String>> {
    // Only upload to Google Files API if using Google endpoint
    let is_google_api = base_url.contains("generativelanguage.googleapis.com");

    if !is_google_api {
        // For non-Google APIs, return None to indicate no upload was performed
        return Ok(None);
    }

    let client = Client::new();
    let file_name = path.file_name().unwrap().to_str().unwrap().to_string();

    let content = tokio::fs::read(path).await?;
    let part = reqwest::multipart::Part::bytes(content)
        .file_name(file_name)
        .mime_str("audio/ogg")?;

    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("file", "{\"display_name\": \"Audio Upload\"}");

    let response = client
        .post(format!(
            "https://generativelanguage.googleapis.com/upload/v1beta/files?key={}",
            api_key
        ))
        .multipart(form)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Upload failed: {}", response.text().await?));
    }

    let upload_res: UploadResponseCorrect = response.json().await?;
    let file_resource = upload_res.file;

    let mut state = file_resource.state;
    let name = file_resource.name;
    let uri = file_resource.uri;

    // Poll if not active
    while state == "PROCESSING" {
        sleep(Duration::from_secs(2)).await;

        let get_res = client
            .get(format!(
                "https://generativelanguage.googleapis.com/v1beta/{}?key={}",
                name, api_key
            ))
            .send()
            .await?;

        if !get_res.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to poll file status: {}",
                get_res.text().await?
            ));
        }

        let poll_res: FileResource = get_res.json().await?;
        state = poll_res.state;

        if state == "FAILED" {
            return Err(anyhow::anyhow!("File processing failed"));
        }
    }

    Ok(Some(uri))
}
