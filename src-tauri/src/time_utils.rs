use anyhow::{anyhow, Result};

/// Raw timestamp parser without correction logic - used internally.
/// Replicates the logic from the provided Python snippet.
pub fn parse_timestamp_to_seconds_raw(ts: &str) -> Result<f64> {
    let _original_ts = ts;
    let mut ts = ts.trim().replace(" ", "");

    if ts.is_empty() {
        return Err(anyhow!("Empty timestamp"));
    }

    let mut milliseconds = 0.0;
    let mut new_ts_string: Option<String> = None;

    if let Some((main_part, ms_part)) = ts.rsplit_once('.') {
        // Pad ms_part to 3 digits to handle cases like .1 -> 100ms
        let ms_part_padded = format!("{:0<3}", ms_part);
        let ms_val = ms_part_padded
            .chars()
            .take(3)
            .collect::<String>()
            .parse::<f64>()
            .unwrap_or(0.0);
        milliseconds = ms_val / 1000.0;
        new_ts_string = Some(main_part.to_string());
    } else if let Some((prefix, last_part)) = ts.rsplit_once(':') {
        // Handle cases like HH:MM:SSmmm (no dot)
        if last_part.len() > 2 && last_part.chars().all(|c| c.is_ascii_digit()) {
            let split_idx = last_part.len().saturating_sub(3);
            let seconds_str = &last_part[..split_idx];
            let milliseconds_str = &last_part[split_idx..];

            if !seconds_str.is_empty() {
                new_ts_string = Some(format!("{}:{}", prefix, seconds_str));
            } else {
                // case like MM:mmm where seconds part is empty?
                // Python: if seconds_str: ts = f"{prefix}:{seconds_str}" else: ts = prefix
                new_ts_string = Some(prefix.to_string());
            }

            let ms_val = milliseconds_str.parse::<f64>().unwrap_or(0.0);
            milliseconds = ms_val / 1000.0;
        }
    }

    if let Some(s) = new_ts_string {
        ts = s;
    }

    let parts: Vec<&str> = ts.split(':').collect();
    let mut h = 0;
    let mut m = 0;
    let mut s;

    if parts.len() == 3 {
        h = parts[0]
            .parse::<i32>()
            .map_err(|_| anyhow!("Invalid hour"))?;
        m = parts[1]
            .parse::<i32>()
            .map_err(|_| anyhow!("Invalid minute"))?;
        s = parts[2]
            .parse::<i32>()
            .map_err(|_| anyhow!("Invalid second"))?;
    } else if parts.len() == 2 {
        m = parts[0]
            .parse::<i32>()
            .map_err(|_| anyhow!("Invalid minute"))?;
        s = parts[1]
            .parse::<i32>()
            .map_err(|_| anyhow!("Invalid second"))?;
    } else if parts.len() == 1 && !parts[0].is_empty() {
        s = parts[0]
            .parse::<i32>()
            .map_err(|_| anyhow!("Invalid second"))?;
    } else if parts.len() == 1 && parts[0].is_empty() {
        // This can happen if the format is .mmm (ts became empty after splitting dot)
        s = 0;
    } else {
        return Err(anyhow!("Invalid timestamp format"));
    }

    // Fix common AI timestamp errors before validation
    // Convert 60 seconds to next minute
    if s >= 60 {
        let extra_minutes = s / 60;
        s = s % 60;
        m += extra_minutes;
    }

    // Convert 60+ minutes to next hour
    if m >= 60 {
        let extra_hours = m / 60;
        m = m % 60;
        h += extra_hours;
    }

    // Validate ranges
    if h < 0 || m < 0 || s < 0 {
        return Err(anyhow!("Negative time values not allowed"));
    }
    // Note: Python code checks m >= 60 and s >= 60 *after* fixing.
    // Since we fixed them above, these checks should technically pass unless logic is wrong,
    // but the python code had them.
    if m >= 60 {
        return Err(anyhow!("Minutes must be 0-59, got {}", m));
    }
    if s >= 60 {
        return Err(anyhow!("Seconds must be 0-59, got {}", s));
    }
    if milliseconds >= 1.0 {
        return Err(anyhow!(
            "Milliseconds must be < 1000, got {}",
            milliseconds * 1000.0
        ));
    }

    Ok((h as f64 * 3600.0) + (m as f64 * 60.0) + (s as f64) + milliseconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_normal() {
        assert_eq!(parse_timestamp_to_seconds_raw("00:00:10").unwrap(), 10.0);
        assert_eq!(parse_timestamp_to_seconds_raw("01:00:00").unwrap(), 3600.0);
        assert_eq!(parse_timestamp_to_seconds_raw("00:01:30").unwrap(), 90.0);
    }

    #[test]
    fn test_parse_milliseconds() {
        assert_eq!(parse_timestamp_to_seconds_raw("00:00:10.5").unwrap(), 10.5);
        assert_eq!(
            parse_timestamp_to_seconds_raw("00:00:10.500").unwrap(),
            10.5
        );
        assert_eq!(
            parse_timestamp_to_seconds_raw("00:00:10.05").unwrap(),
            10.05
        );
    }

    #[test]
    fn test_parse_overflow_fix() {
        // 60 seconds -> 1 minute
        assert_eq!(parse_timestamp_to_seconds_raw("00:00:60").unwrap(), 60.0);
        // 90 seconds -> 1 minute 30 seconds
        assert_eq!(parse_timestamp_to_seconds_raw("00:00:90").unwrap(), 90.0);
        // 60 minutes -> 1 hour
        assert_eq!(parse_timestamp_to_seconds_raw("00:60:00").unwrap(), 3600.0);
    }

    #[test]
    fn test_parse_weird_formats() {
        // .mmm
        assert_eq!(parse_timestamp_to_seconds_raw(".500").unwrap(), 0.5);
        // HH:MM:SSmmm (no dot)
        // 00:00:01500 -> 1.5s
        assert_eq!(parse_timestamp_to_seconds_raw("00:00:01500").unwrap(), 1.5);
    }

    #[test]
    fn test_errors() {
        assert!(parse_timestamp_to_seconds_raw("").is_err());
        assert!(parse_timestamp_to_seconds_raw("abc").is_err());
        assert!(parse_timestamp_to_seconds_raw("-10:00").is_err());
    }
}
