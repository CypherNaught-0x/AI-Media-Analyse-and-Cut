//! Downloads of model files, pinned to a repository revision and verified by
//! SHA-256, so a changed or corrupted upstream file is rejected instead of
//! being loaded. Interrupted downloads resume where they stopped.

use anyhow::{anyhow, Context, Result};
use reqwest::header::RANGE;
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Where pinned files come from: `{base}/{repo}/resolve/{revision}/{path}`.
pub(crate) const HUGGING_FACE: &str = "https://huggingface.co";

/// One file of a model repository at a fixed revision.
pub(crate) struct PinnedFile {
    pub repo: &'static str,
    /// Commit the file is taken from; `main` would let it change underneath us.
    pub revision: &'static str,
    /// Path inside the repository.
    pub path: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

impl PinnedFile {
    fn url(&self, base: &str) -> String {
        format!(
            "{base}/{}/resolve/{}/{}?download=1",
            self.repo, self.revision, self.path
        )
    }

    /// File name to store it under.
    pub fn file_name(&self) -> &'static str {
        self.path.rsplit('/').next().unwrap_or(self.path)
    }
}

/// `altunenes/parakeet-rs` at the commit the app was tested with
/// (2026-09-23). Hashes were checked against the repository's LFS metadata.
const PARAKEET_RS: &str = "altunenes/parakeet-rs";
const PARAKEET_RS_REVISION: &str = "4d2a8bc71f5c896ec40faa59732e6716295edaf2";

pub(crate) const SORTFORMER_V2: PinnedFile = PinnedFile {
    repo: PARAKEET_RS,
    revision: PARAKEET_RS_REVISION,
    path: "diar_streaming_sortformer_4spk-v2.onnx",
    sha256: "cc520901a8cc25a8d7f7c2c8561a465709b67dd4f1df0572a97530087f3fbc73",
    size: 492_243_002,
};

pub(crate) const PARAKEET_TDT_INT8: [PinnedFile; 3] = [
    PinnedFile {
        repo: PARAKEET_RS,
        revision: PARAKEET_RS_REVISION,
        path: "tdt/encoder-model.int8.onnx",
        sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
        size: 652_183_999,
    },
    PinnedFile {
        repo: PARAKEET_RS,
        revision: PARAKEET_RS_REVISION,
        path: "tdt/decoder_joint-model.int8.onnx",
        sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
        size: 18_202_004,
    },
    PinnedFile {
        repo: PARAKEET_RS,
        revision: PARAKEET_RS_REVISION,
        path: "tdt/vocab.txt",
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
        size: 93_939,
    },
];

/// Make sure `destination` holds `file`, downloading it from `base` if needed.
///
/// A file already present with the expected size is trusted (hashing 650 MB
/// on every start would be slow); every download is hashed before it is moved
/// into place. A leftover `.partial` from an interrupted download is resumed
/// with an HTTP range request.
pub(crate) async fn ensure_pinned_file(
    client: &reqwest::Client,
    base: &str,
    file: &PinnedFile,
    destination: &Path,
    on_progress: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    if tokio::fs::metadata(destination)
        .await
        .is_ok_and(|meta| meta.len() == file.size)
    {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let label = file.file_name();
    let partial = partial_path(destination);
    let mut hasher = Sha256::new();
    let mut have = hash_existing(&partial, file.size, &mut hasher).await?;

    let mut request = client.get(file.url(base));
    if have > 0 {
        request = request.header(RANGE, format!("bytes={have}-"));
        on_progress(&format!("Resuming download of {label}..."));
    } else {
        on_progress(&format!("Downloading {label}..."));
    }
    let mut response = request
        .send()
        .await
        .with_context(|| format!("Failed to start download for {label}"))?
        .error_for_status()
        .with_context(|| format!("Failed to download {label}"))?;

    let mut output = if have > 0 && response.status() == StatusCode::PARTIAL_CONTENT {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&partial)
            .await?
    } else {
        // The server ignored the range: start over.
        hasher = Sha256::new();
        have = 0;
        tokio::fs::File::create(&partial)
            .await
            .with_context(|| format!("Failed to create '{}'", partial.display()))?
    };

    let mut last_reported = 0;
    while let Some(chunk) = response.chunk().await? {
        output.write_all(&chunk).await?;
        hasher.update(&chunk);
        have += chunk.len() as u64;
        let percent = have * 100 / file.size.max(1);
        if percent >= last_reported + 10 {
            last_reported = percent.min(100);
            on_progress(&format!("Downloading {label}... {last_reported}%"));
        }
    }
    output.flush().await?;
    drop(output);

    let actual = format!("{:x}", hasher.finalize());
    if actual != file.sha256 || have != file.size {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(anyhow!(
            "Downloaded {label} failed verification (expected {} bytes, sha256 {}; got {have} bytes, sha256 {actual}). \
             The download was discarded; try again.",
            file.size,
            file.sha256
        ));
    }

    tokio::fs::rename(&partial, destination)
        .await
        .with_context(|| format!("Failed to finalize '{}'", destination.display()))?;
    on_progress(&format!("Downloaded {label}"));
    Ok(())
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_owned();
    name.push(".partial");
    PathBuf::from(name)
}

/// Feed an existing partial download into `hasher`; returns its length, or 0
/// (after deleting it) when it can't be a prefix of the file.
async fn hash_existing(partial: &Path, expected_size: u64, hasher: &mut Sha256) -> Result<u64> {
    let Ok(mut existing) = tokio::fs::File::open(partial).await else {
        return Ok(0);
    };
    let length = existing.metadata().await?.len();
    if length == 0 || length >= expected_size {
        drop(existing);
        let _ = tokio::fs::remove_file(partial).await;
        return Ok(0);
    }
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = existing.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = b"pretend this is a large onnx model";

    fn pinned(sha256: &'static str) -> PinnedFile {
        PinnedFile {
            repo: "org/repo",
            revision: "abc123",
            path: "dir/model.onnx",
            sha256,
            size: BODY.len() as u64,
        }
    }

    fn body_sha256() -> &'static str {
        let hash = format!("{:x}", Sha256::digest(BODY));
        Box::leak(hash.into_boxed_str())
    }

    #[tokio::test]
    async fn downloads_the_pinned_revision_and_verifies_it() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/org/repo/resolve/abc123/dir/model.onnx?download=1")
            .with_body(BODY)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("model.onnx");

        ensure_pinned_file(
            &reqwest::Client::new(),
            &server.url(),
            &pinned(body_sha256()),
            &destination,
            &|_| {},
        )
        .await
        .unwrap();

        mock.assert_async().await;
        assert_eq!(std::fs::read(&destination).unwrap(), BODY);
        assert!(!partial_path(&destination).exists());
    }

    #[tokio::test]
    async fn rejects_a_file_with_the_wrong_hash() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/org/repo/resolve/abc123/dir/model.onnx?download=1")
            .with_body(BODY)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("model.onnx");
        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";

        let error = ensure_pinned_file(
            &reqwest::Client::new(),
            &server.url(),
            &pinned(wrong),
            &destination,
            &|_| {},
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("failed verification"), "{error}");
        assert!(!destination.exists());
        assert!(!partial_path(&destination).exists());
    }

    #[tokio::test]
    async fn resumes_an_interrupted_download() {
        let split = 10;
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/org/repo/resolve/abc123/dir/model.onnx?download=1")
            .match_header("range", format!("bytes={split}-").as_str())
            .with_status(206)
            .with_body(&BODY[split..])
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("model.onnx");
        std::fs::write(partial_path(&destination), &BODY[..split]).unwrap();

        ensure_pinned_file(
            &reqwest::Client::new(),
            &server.url(),
            &pinned(body_sha256()),
            &destination,
            &|_| {},
        )
        .await
        .unwrap();

        mock.assert_async().await;
        assert_eq!(std::fs::read(&destination).unwrap(), BODY);
    }

    #[tokio::test]
    async fn trusts_an_existing_file_of_the_right_size() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("model.onnx");
        std::fs::write(&destination, BODY).unwrap();

        // No server: any request would fail.
        ensure_pinned_file(
            &reqwest::Client::new(),
            "http://127.0.0.1:9",
            &pinned(body_sha256()),
            &destination,
            &|_| {},
        )
        .await
        .unwrap();
    }

    #[test]
    fn pinned_urls_use_the_revision_not_main() {
        assert_eq!(
            PARAKEET_TDT_INT8[0].url(HUGGING_FACE),
            "https://huggingface.co/altunenes/parakeet-rs/resolve/4d2a8bc71f5c896ec40faa59732e6716295edaf2/tdt/encoder-model.int8.onnx?download=1"
        );
        assert_eq!(
            SORTFORMER_V2.file_name(),
            "diar_streaming_sortformer_4spk-v2.onnx"
        );
    }
}
