use std::future::Future;
use std::time::Duration;
use log::{info, warn, error};

/// Retry configuration for API calls
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Initial delay between retries
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Exponential backoff multiplier
    pub backoff_multiplier: f64,
    /// HTTP status codes that should trigger a retry
    pub retryable_status_codes: Vec<u16>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            retryable_status_codes: vec![429, 500, 502, 503, 504],
        }
    }
}

impl RetryConfig {
    /// Create a conservative retry config for important operations
    pub fn conservative() -> Self {
        Self {
            max_attempts: 5,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            retryable_status_codes: vec![429, 500, 502, 503, 504],
        }
    }

    /// Create an aggressive retry config for fast operations
    pub fn aggressive() -> Self {
        Self {
            max_attempts: 2,
            initial_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 1.5,
            retryable_status_codes: vec![429, 500, 502, 503, 504],
        }
    }
}

/// Error types that can trigger a retry
#[derive(Debug)]
pub enum RetryableError {
    /// Network-level errors (connection refused, timeout, etc.)
    Network(String),
    /// HTTP errors with specific status codes
    Http { status: u16, message: String },
    /// Rate limiting
    RateLimited { retry_after: Option<Duration> },
    /// Server errors
    Server(String),
}

impl RetryableError {
    /// Check if this error should trigger a retry based on the config
    pub fn is_retryable(&self, config: &RetryConfig) -> bool {
        match self {
            RetryableError::Network(_) => true,
            RetryableError::Http { status, .. } => {
                config.retryable_status_codes.contains(status)
            }
            RetryableError::RateLimited { .. } => true,
            RetryableError::Server(_) => true,
        }
    }

    /// Get the recommended delay before retrying
    pub fn retry_delay(&self, attempt: u32, config: &RetryConfig) -> Duration {
        let base_delay = match self {
            RetryableError::RateLimited { retry_after } => {
                retry_after.unwrap_or(config.initial_delay)
            }
            _ => {
                let exponential = config.initial_delay.as_millis() as f64
                    * config.backoff_multiplier.powi(attempt as i32 - 1);
                let capped = exponential.min(config.max_delay.as_millis() as f64);
                Duration::from_millis(capped as u64)
            }
        };

        // Add jitter to prevent thundering herd
        let jitter = Duration::from_millis(fastrand::u64(0..=100));
        base_delay + jitter
    }
}

impl std::fmt::Display for RetryableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetryableError::Network(msg) => write!(f, "Network error: {}", msg),
            RetryableError::Http { status, message } => {
                write!(f, "HTTP error {}: {}", status, message)
            }
            RetryableError::RateLimited { retry_after } => {
                if let Some(delay) = retry_after {
                    write!(f, "Rate limited. Retry after {:?}", delay)
                } else {
                    write!(f, "Rate limited")
                }
            }
            RetryableError::Server(msg) => write!(f, "Server error: {}", msg),
        }
    }
}

impl std::error::Error for RetryableError {}

/// Execute an async operation with retry logic
pub async fn retry_with_backoff<F, Fut, T, E>(
    operation: F,
    config: &RetryConfig,
    operation_name: &str,
) -> Result<T, RetryableError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Into<RetryableError>,
{
    let mut last_error: Option<RetryableError> = None;

    for attempt in 1..=config.max_attempts {
        match operation().await {
            Ok(result) => {
                if attempt > 1 {
                    info!("{} succeeded after {} attempts", operation_name, attempt);
                }
                return Ok(result);
            }
            Err(err) => {
                let retryable_err: RetryableError = err.into();

                if !retryable_err.is_retryable(config) {
                    return Err(retryable_err);
                }

                if attempt < config.max_attempts {
                    let delay = retryable_err.retry_delay(attempt, config);
                    warn!(
                        "{} failed (attempt {}/{}): {}. Retrying in {:?}...",
                        operation_name, attempt, config.max_attempts, retryable_err, delay
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    error!(
                        "{} failed after {} attempts: {}",
                        operation_name, config.max_attempts, retryable_err
                    );
                }

                last_error = Some(retryable_err);
            }
        }
    }

    Err(last_error.expect("last_error should be set if we exit the loop"))
}

/// Convert anyhow::Error to RetryableError
impl From<anyhow::Error> for RetryableError {
    fn from(err: anyhow::Error) -> Self {
        let err_string = err.to_string().to_lowercase();

        if err_string.contains("timeout")
            || err_string.contains("timed out")
            || err_string.contains("deadline")
        {
            RetryableError::Network(format!("Timeout: {}", err))
        } else if err_string.contains("connection")
            || err_string.contains("refused")
            || err_string.contains("reset")
            || err_string.contains("dns")
        {
            RetryableError::Network(format!("Connection error: {}", err))
        } else if err_string.contains("rate limit")
            || err_string.contains("too many requests")
            || err_string.contains("429")
        {
            RetryableError::RateLimited { retry_after: None }
        } else {
            RetryableError::Server(err.to_string())
        }
    }
}

/// Convert reqwest::Error to RetryableError
impl From<reqwest::Error> for RetryableError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            RetryableError::Network(format!("Request timeout: {}", err))
        } else if err.is_connect() {
            RetryableError::Network(format!("Connection error: {}", err))
        } else if err.is_request() {
            RetryableError::Network(format!("Request error: {}", err))
        } else {
            RetryableError::Server(format!("HTTP error: {}", err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.initial_delay, Duration::from_millis(500));
        assert_eq!(config.max_delay, Duration::from_secs(30));
        assert_eq!(config.backoff_multiplier, 2.0);
    }

    #[test]
    fn test_retry_config_conservative() {
        let config = RetryConfig::conservative();
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(60));
    }

    #[test]
    fn test_retry_config_aggressive() {
        let config = RetryConfig::aggressive();
        assert_eq!(config.max_attempts, 2);
        assert_eq!(config.initial_delay, Duration::from_millis(250));
        assert_eq!(config.max_delay, Duration::from_secs(5));
    }

    #[test]
    fn test_is_retryable() {
        let config = RetryConfig::default();

        assert!(
            RetryableError::Network("test".to_string()).is_retryable(&config),
            "Network errors should be retryable"
        );

        assert!(
            RetryableError::Http {
                status: 429,
                message: "Too Many Requests".to_string()
            }
            .is_retryable(&config),
            "429 errors should be retryable"
        );

        assert!(
            RetryableError::Http {
                status: 500,
                message: "Internal Server Error".to_string()
            }
            .is_retryable(&config),
            "500 errors should be retryable"
        );

        assert!(
            !RetryableError::Http {
                status: 400,
                message: "Bad Request".to_string()
            }
            .is_retryable(&config),
            "400 errors should not be retryable"
        );
    }

    #[test]
    fn test_retry_delay() {
        let config = RetryConfig::default();

        let network_err = RetryableError::Network("test".to_string());
        let delay1 = network_err.retry_delay(1, &config);
        let delay2 = network_err.retry_delay(2, &config);

        // Delay should increase with attempts
        assert!(
            delay2 >= delay1,
            "Second attempt delay should be >= first attempt"
        );

        // Test rate limited with specific retry_after
        let rate_limited = RetryableError::RateLimited {
            retry_after: Some(Duration::from_secs(5)),
        };
        let delay = rate_limited.retry_delay(1, &config);
        assert!(
            delay >= Duration::from_secs(5),
            "Rate limited delay should respect retry_after"
        );
    }

    #[tokio::test]
    async fn test_retry_with_backoff_success() {
        let config = RetryConfig::aggressive();
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let result = retry_with_backoff(
            || {
                let attempts_clone = attempts.clone();
                async move {
                    let count = attempts_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if count < 1 {
                        Err::<i32, RetryableError>(RetryableError::Network("test".to_string()))
                    } else {
                        Ok(42)
                    }
                }
            },
            &config,
            "test_operation",
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_exhausted() {
        let config = RetryConfig {
            max_attempts: 2,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            backoff_multiplier: 1.0,
            retryable_status_codes: vec![500],
        };

        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let result = retry_with_backoff(
            || {
                let attempts_clone = attempts.clone();
                async move {
                    attempts_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err::<i32, RetryableError>(RetryableError::Network("always fails".to_string()))
                }
            },
            &config,
            "test_operation",
        )
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_non_retryable() {
        let config = RetryConfig::default();
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let result = retry_with_backoff(
            || {
                let attempts_clone = attempts.clone();
                async move {
                    attempts_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err::<i32, RetryableError>(RetryableError::Http {
                        status: 400,
                        message: "Bad Request".to_string(),
                    })
                }
            },
            &config,
            "test_operation",
        )
        .await;

        assert!(result.is_err());
        // Should not retry non-retryable errors
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
