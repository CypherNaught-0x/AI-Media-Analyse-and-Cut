//! `media://` — serves local media to the webview's `<video>`/`<audio>`.
//!
//! Tauri's built-in `asset://` protocol cuts every range response to 1000 KiB.
//! WebKit's MP4 demuxer requests each box of the `moov` index with one
//! explicit range and drops a track when the answer comes back short, so for
//! long recordings (a 4 MB `moov` is normal for an hour of video) previews
//! played without sound. This protocol answers explicit ranges in full (up to
//! [`MAX_EXPLICIT_RANGE`]) and gives open-ended requests a streaming-sized
//! chunk.
//!
//! Only files inside folders the app granted for playback
//! (`allow_media_access`) or the media cache are served.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use tauri::http::{header, Request, Response, StatusCode};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Largest explicit range answered in one response.
const MAX_EXPLICIT_RANGE: u64 = 64 * 1024 * 1024;
/// Response size for open-ended (`bytes=N-`) or range-less requests.
const STREAM_CHUNK: u64 = 4 * 1024 * 1024;

/// Folders whose files may be served.
#[derive(Default)]
pub(crate) struct MediaScope {
    roots: RwLock<Vec<PathBuf>>,
}

impl MediaScope {
    pub(crate) fn allow(&self, folder: &Path) {
        let folder = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        let mut roots = self.roots.write().expect("media scope poisoned");
        if !roots.contains(&folder) {
            roots.push(folder);
        }
    }

    /// The file, resolved, if it lies inside an allowed folder.
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        let resolved = std::fs::canonicalize(path).ok()?;
        let roots = self.roots.read().expect("media scope poisoned");
        roots
            .iter()
            .any(|root| resolved.starts_with(root))
            .then_some(resolved)
    }
}

/// Which bytes to send: `start..=end`, and whether that is a partial (206)
/// response. `None` means the range can't be satisfied (416).
pub(crate) fn plan_range(range_header: Option<&str>, len: u64) -> Option<(u64, u64, bool)> {
    if len == 0 {
        return None;
    }
    let Some(spec) = range_header.and_then(|value| value.trim().strip_prefix("bytes=")) else {
        // No range: small files whole, large ones as a first chunk.
        return Some(if len <= STREAM_CHUNK {
            (0, len - 1, false)
        } else {
            (0, STREAM_CHUNK - 1, true)
        });
    };
    // Only the first range of a multi-range request is honoured.
    let spec = spec.split(',').next()?.trim();
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        // Suffix range: the last N bytes.
        ("", suffix) => {
            let suffix: u64 = suffix.parse().ok()?;
            (len.saturating_sub(suffix.min(MAX_EXPLICIT_RANGE)), len - 1)
        }
        (start, "") => {
            let start: u64 = start.parse().ok()?;
            (start, (start + STREAM_CHUNK - 1).min(len - 1))
        }
        (start, end) => {
            let start: u64 = start.parse().ok()?;
            let end: u64 = end.parse().ok()?;
            if end < start {
                return None;
            }
            (start, end.min(len - 1).min(start + MAX_EXPLICIT_RANGE - 1))
        }
    };
    (start < len).then_some((start, end, true))
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("m4a") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg" | "opus") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("aac") => "audio/aac",
        _ => "application/octet-stream",
    }
}

fn status_only(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Vec::new())
        .expect("static response is valid")
}

/// The file path a `media://` URL refers to (as built by the frontend's
/// `convertFileSrc(path, 'media')`: the percent-encoded path as URL path).
fn requested_path(uri: &tauri::http::Uri) -> Option<PathBuf> {
    let raw = uri.path().trim_start_matches('/');
    let decoded = percent_decode(raw)?;
    Some(PathBuf::from(decoded))
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = input.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Answer one request. `scope` decides which files may be served.
pub(crate) async fn serve(scope: &MediaScope, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(path) = requested_path(request.uri()).and_then(|path| scope.resolve(&path)) else {
        return status_only(StatusCode::FORBIDDEN);
    };
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return status_only(StatusCode::NOT_FOUND);
    };
    let Ok(len) = file.metadata().await.map(|meta| meta.len()) else {
        return status_only(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let Some((start, end, partial)) = plan_range(range, len) else {
        let mut response = status_only(StatusCode::RANGE_NOT_SATISFIABLE);
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            format!("bytes */{len}").parse().expect("valid header"),
        );
        return response;
    };

    let mut body = vec![0u8; (end - start + 1) as usize];
    if file.seek(SeekFrom::Start(start)).await.is_err() || file.read_exact(&mut body).await.is_err()
    {
        return status_only(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let mut builder = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, content_type(&path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, body.len())
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if partial {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    builder.body(body).expect("response headers are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn explicit_ranges_are_answered_in_full() {
        // A 4 MB moov request: the asset protocol would cut this at 1000 KiB.
        assert_eq!(
            plan_range(Some("bytes=32-4088973"), 800 * MB),
            Some((32, 4_088_973, true))
        );
        assert_eq!(plan_range(Some("bytes=0-1"), 100), Some((0, 1, true)));
        // Clamped to the file and to the per-response maximum.
        assert_eq!(plan_range(Some("bytes=90-500"), 100), Some((90, 99, true)));
        assert_eq!(
            plan_range(Some("bytes=0-999999999"), 800 * MB),
            Some((0, MAX_EXPLICIT_RANGE - 1, true))
        );
    }

    #[test]
    fn open_ended_and_suffix_ranges() {
        assert_eq!(
            plan_range(Some("bytes=1000-"), 800 * MB),
            Some((1000, 1000 + STREAM_CHUNK - 1, true))
        );
        assert_eq!(plan_range(Some("bytes=-10"), 100), Some((90, 99, true)));
        assert_eq!(plan_range(None, 100), Some((0, 99, false)));
        assert_eq!(
            plan_range(None, 800 * MB),
            Some((0, STREAM_CHUNK - 1, true))
        );
    }

    #[test]
    fn unsatisfiable_ranges() {
        assert_eq!(plan_range(Some("bytes=100-"), 100), None);
        assert_eq!(plan_range(Some("bytes=50-10"), 100), None);
        assert_eq!(plan_range(Some("bytes=x-1"), 100), None);
        assert_eq!(plan_range(None, 0), None);
    }

    fn request(uri: &str, range: Option<&str>) -> Request<Vec<u8>> {
        let mut builder = Request::builder().uri(uri);
        if let Some(range) = range {
            builder = builder.header(header::RANGE, range);
        }
        builder.body(Vec::new()).unwrap()
    }

    fn encode(path: &Path) -> String {
        path.to_string_lossy()
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn serves_granted_files_with_range_headers() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("my talk.mp4");
        std::fs::write(&file, (0u8..=255).collect::<Vec<_>>()).unwrap();
        let scope = MediaScope::default();
        scope.allow(dir.path());

        let url = format!("media://localhost/{}", encode(&file));
        let response = serve(&scope, &request(&url, Some("bytes=10-19"))).await;

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.body(), &(10u8..=19).collect::<Vec<_>>());
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 10-19/256");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
    }

    #[tokio::test]
    async fn refuses_files_outside_the_granted_folders() {
        let dir = tempfile::tempdir().unwrap();
        let granted = dir.path().join("granted");
        std::fs::create_dir_all(&granted).unwrap();
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, b"nope").unwrap();
        let scope = MediaScope::default();
        scope.allow(&granted);

        let direct = format!("media://localhost/{}", encode(&secret));
        let escaping = format!(
            "media://localhost/{}",
            encode(&granted.join("..").join("secret.txt"))
        );
        for url in [direct, escaping] {
            let response = serve(&scope, &request(&url, None)).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{url}");
            assert!(response.body().is_empty());
        }
    }
}
