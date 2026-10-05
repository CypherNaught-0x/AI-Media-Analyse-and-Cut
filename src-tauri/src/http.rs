//! The HTTP client shared by all outgoing requests (LLM APIs, uploads, model
//! downloads), so connections are reused and every request has timeouts.

use std::sync::OnceLock;
use std::time::Duration;

/// Connection setup must be quick; a response may legitimately take minutes
/// (audio analysis), so only a long silence on an open connection times out.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) fn http_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .build()
                .expect("HTTP client configuration is valid")
        })
        // Cheap: clones share the connection pool.
        .clone()
}
