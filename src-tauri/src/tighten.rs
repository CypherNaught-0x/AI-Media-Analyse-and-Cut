//! Tightening clips (shorts phase S4): shorter pauses, no fillers, no
//! stutters. The result is more, shorter source ranges per clip; the cuts
//! between them are jump cuts that the renderer can dress up (punch-ins).

use crate::captions::TimedWord;
use crate::pauses::detect_pauses;
use crate::run_control::RunControl;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How hard to tighten, as in the plan's intensity presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Intensity {
    Off,
    Chill,
    Punchy,
    Hyper,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TightenSettings {
    /// Longest pause kept (seconds); longer ones shrink to this.
    pub max_pause: f64,
    pub fillers: bool,
    pub repeats: bool,
    /// Cuts that save less than this aren't worth a jump cut.
    pub min_cut: f64,
    /// Kept stretches without speech shorter than this are dropped.
    pub min_keep: f64,
}

impl Intensity {
    pub(crate) fn settings(self) -> Option<TightenSettings> {
        let base = TightenSettings {
            max_pause: 0.35,
            fillers: true,
            repeats: false,
            min_cut: 0.15,
            min_keep: 0.5,
        };
        match self {
            Intensity::Off => None,
            Intensity::Chill => Some(base),
            Intensity::Punchy => Some(TightenSettings {
                max_pause: 0.18,
                repeats: true,
                min_cut: 0.12,
                ..base
            }),
            Intensity::Hyper => Some(TightenSettings {
                max_pause: 0.09,
                repeats: true,
                min_cut: 0.1,
                ..base
            }),
        }
    }
}

/// Hesitation sounds, German and English. Not "er" or "eh": in German those
/// are words ("he", "anyway").
const FILLERS: &[&str] = &[
    "äh", "ähm", "ähh", "äähm", "öh", "öhm", "ehm", "hm", "hmm", "mhm", "uh", "uhm", "um", "umm",
    "erm",
];

/// Doubled words that are often grammatical in German ("die, die ich…",
/// "dass das"), so a repeat of them isn't a stutter.
const GRAMMATICAL_DOUBLES: &[&str] = &["die", "der", "das", "dass", "sie", "es", "wie"];

fn normalized(text: &str) -> String {
    text.trim()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn is_filler(word: &TimedWord) -> bool {
    FILLERS.contains(&normalized(&word.text).as_str())
}

/// Stutters: the first of two equal spoken words in a row (fillers between
/// them don't count), unless the double is grammatical or a clause ends.
fn stutters<'a>(words: &[&'a TimedWord]) -> Vec<&'a TimedWord> {
    let spoken: Vec<&TimedWord> = words.iter().copied().filter(|w| !is_filler(w)).collect();
    spoken
        .windows(2)
        .filter(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let text = normalized(&a.text);
            text.chars().count() >= 2
                && text == normalized(&b.text)
                && !GRAMMATICAL_DOUBLES.contains(&text.as_str())
                && !a.text.trim_end().ends_with([',', '.', '?', '!', ';', ':'])
        })
        .map(|pair| pair[0])
        .collect()
}

/// Words tightening takes out on purpose (fillers, stutters), so captions
/// can leave them out too.
pub(crate) fn removed_words(words: &[TimedWord], intensity: Intensity) -> Vec<TimedWord> {
    let Some(settings) = intensity.settings() else {
        return Vec::new();
    };
    let all: Vec<&TimedWord> = words.iter().collect();
    let mut removed: Vec<TimedWord> = Vec::new();
    if settings.fillers {
        removed.extend(words.iter().filter(|w| is_filler(w)).cloned());
    }
    if settings.repeats {
        removed.extend(stutters(&all).into_iter().cloned());
    }
    removed
}

/// Spans of `range` to remove.
fn removals(
    range: (f64, f64),
    pauses: &[(f64, f64)],
    words: &[&TimedWord],
    settings: &TightenSettings,
) -> Vec<(f64, f64)> {
    let (start, end) = range;
    let air = settings.max_pause / 2.0;
    let mut spans = Vec::new();
    for &(a, b) in pauses {
        let (a, b) = (a.max(start), b.min(end));
        if b - a <= settings.max_pause {
            continue;
        }
        // At the clip's edges keep a breath before the first word and after
        // the last one; between words, half the allowed pause on each side.
        let from = if a <= start + 1e-6 { start } else { a + air };
        let to = if b >= end - 1e-6 { end } else { b - air };
        spans.push((from, to));
    }
    if settings.fillers {
        spans.extend(
            words
                .iter()
                .filter(|word| is_filler(word))
                .map(|word| (word.start.max(start), word.end.min(end))),
        );
    }
    if settings.repeats {
        // "Ich ähm ich" is a stutter too: `stutters` compares across fillers.
        // The cut runs from the first word to the start of the next spoken
        // one, taking any filler in between with it.
        let spoken: Vec<&TimedWord> = words.iter().copied().filter(|w| !is_filler(w)).collect();
        for first in stutters(words) {
            let next = spoken
                .iter()
                .find(|w| w.start >= first.end - 1e-9)
                .map_or(first.end, |w| w.start);
            spans.push((first.start.max(start), next.min(end)));
        }
    }
    spans.retain(|(a, b)| b - a >= settings.min_cut);
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Union of overlapping spans.
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.0 <= last.1 => last.1 = last.1.max(span.1),
            _ => merged.push(span),
        }
    }
    merged
}

/// The kept parts of `range` (source seconds) after tightening.
pub(crate) fn tighten_range(
    range: (f64, f64),
    pauses: &[(f64, f64)],
    words: &[TimedWord],
    settings: &TightenSettings,
) -> Vec<(f64, f64)> {
    let (start, end) = range;
    let inside: Vec<&TimedWord> = words
        .iter()
        .filter(|w| w.end > start && w.start < end)
        .collect();
    let mut kept = Vec::new();
    let mut cursor = start;
    for (a, b) in removals(range, pauses, &inside, settings) {
        if a > cursor {
            kept.push((cursor, a));
        }
        cursor = cursor.max(b);
    }
    if cursor < end {
        kept.push((cursor, end));
    }
    // Slivers without speech aren't worth a cut each.
    let speaks = |(a, b): (f64, f64)| {
        inside.iter().any(|w| {
            let middle = (w.start + w.end) / 2.0;
            a <= middle && middle < b && !is_filler(w)
        })
    };
    kept.retain(|&span| span.1 - span.0 >= settings.min_keep || speaks(span));
    if kept.is_empty() {
        // Never tighten a clip away entirely.
        return vec![range];
    }
    kept
}

/// Tighten every range of every clip. Blocks.
pub(crate) fn tighten_clips(
    input: &Path,
    clips: &[Vec<(f64, f64)>],
    words: &[TimedWord],
    intensity: Intensity,
    run: Option<(u64, &RunControl)>,
) -> Result<Vec<Vec<(f64, f64)>>, String> {
    let Some(settings) = intensity.settings() else {
        return Ok(clips.to_vec());
    };
    clips
        .iter()
        .map(|ranges| {
            let mut tightened = Vec::new();
            for &range in ranges {
                let pauses = detect_pauses(input, range.0, range.1, settings.max_pause, run)?;
                tightened.extend(tighten_range(range, &pauses, words, &settings));
            }
            Ok(tightened)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64, end: f64, text: &str) -> TimedWord {
        TimedWord {
            start,
            end,
            text: text.to_string(),
        }
    }

    fn punchy() -> TightenSettings {
        Intensity::Punchy.settings().unwrap()
    }

    fn close(a: &[(f64, f64)], b: &[(f64, f64)]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| (x.0 - y.0).abs() < 1e-6 && (x.1 - y.1).abs() < 1e-6)
    }

    #[test]
    fn long_pauses_shrink_to_the_allowed_air() {
        let words = [word(10.0, 11.0, "Hallo"), word(12.0, 13.0, "zusammen")];
        // A 0.8 s pause between the words: 0.09 s of air stays on each side.
        let kept = tighten_range((10.0, 13.0), &[(11.1, 11.9)], &words, &punchy());
        assert!(close(&kept, &[(10.0, 11.19), (11.81, 13.0)]), "{kept:?}");
    }

    #[test]
    fn short_pauses_and_tiny_savings_are_left_alone() {
        let words = [word(10.0, 11.0, "Hallo"), word(11.2, 13.0, "zusammen")];
        let kept = tighten_range((10.0, 13.0), &[(11.0, 11.15)], &words, &punchy());
        assert!(close(&kept, &[(10.0, 13.0)]), "{kept:?}");
    }

    #[test]
    fn edge_silence_is_trimmed_to_a_breath() {
        let words = [word(10.6, 12.0, "Also")];
        let kept = tighten_range(
            (10.0, 12.5),
            &[(10.0, 10.55), (12.05, 12.5)],
            &words,
            &punchy(),
        );
        // The leading pause is cut down to the 0.09 s of air before the
        // word; the trailing one likewise.
        assert!(close(&kept, &[(10.46, 12.14)]), "{kept:?}");
    }

    #[test]
    fn fillers_and_stutters_go_but_grammatical_doubles_stay() {
        let words = [
            word(0.0, 0.5, "Ich"),
            word(0.5, 0.9, "ähm"),
            word(0.9, 1.3, "ich"),
            word(1.3, 1.7, "glaube,"),
            word(1.7, 2.0, "die"),
            word(2.0, 2.3, "die"),
            word(2.3, 2.6, "das"),
            word(2.6, 3.0, "Wissen"),
            word(3.0, 3.4, "Wissen"),
            word(3.4, 4.0, "haben."),
        ];
        let kept = tighten_range((0.0, 4.0), &[], &words, &punchy());
        // "Ich ähm" → removed up to the repeated "ich"; one "Wissen" goes.
        assert!(close(&kept, &[(0.9, 2.6), (3.0, 4.0)]), "{kept:?}");
        // Chill keeps stutters, still drops the filler.
        let chill = tighten_range(
            (0.0, 4.0),
            &[],
            &words,
            &Intensity::Chill.settings().unwrap(),
        );
        assert!(close(&chill, &[(0.0, 0.5), (0.9, 4.0)]), "{chill:?}");
    }

    #[test]
    fn silent_slivers_between_cuts_are_dropped() {
        let words = [word(0.0, 1.0, "Eins"), word(2.0, 3.0, "zwei")];
        // Two pauses with a 0.2 s quiet bit between them.
        let kept = tighten_range((0.0, 3.0), &[(1.0, 1.4), (1.6, 2.0)], &words, &punchy());
        assert!(close(&kept, &[(0.0, 1.09), (1.91, 3.0)]), "{kept:?}");
    }

    #[test]
    fn removed_words_lists_fillers_and_stutters() {
        let words = [
            word(0.0, 0.5, "Ich"),
            word(0.5, 0.9, "ähm"),
            word(0.9, 1.3, "ich"),
            word(1.3, 1.7, "glaube."),
        ];
        let removed: Vec<String> = removed_words(&words, Intensity::Punchy)
            .into_iter()
            .map(|w| w.text)
            .collect();
        assert_eq!(removed, ["ähm", "Ich"]);
        assert!(removed_words(&words, Intensity::Off).is_empty());
    }

    #[test]
    fn off_changes_nothing() {
        assert!(Intensity::Off.settings().is_none());
    }
}

#[cfg(test)]
mod evaluation {
    use super::*;

    /// Tightens a real range at every intensity and reports what's cut.
    /// `SHORTS_PROFILE_SOURCE=… SHORTS_PROFILE_TRANSCRIPT=… SHORTS_PROFILE_RANGE=872-901
    ///  cargo test --release --lib tightening_a_recording -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn tightening_a_recording() {
        let (Some(source), Some(transcript)) = (
            std::env::var_os("SHORTS_PROFILE_SOURCE"),
            std::env::var_os("SHORTS_PROFILE_TRANSCRIPT"),
        ) else {
            eprintln!("SHORTS_PROFILE_SOURCE / _TRANSCRIPT not set; skipping");
            return;
        };
        let range = std::env::var("SHORTS_PROFILE_RANGE").unwrap_or_else(|_| "872-901".into());
        let (start, end) = range.split_once('-').unwrap();
        let range: (f64, f64) = (start.parse().unwrap(), end.parse().unwrap());
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(transcript).unwrap()).unwrap();
        let words: Vec<TimedWord> = json["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|segment| segment["words"].as_array().cloned().unwrap_or_default())
            .map(|word| TimedWord {
                start: crate::time_utils::parse_time(word["start"].as_str().unwrap()),
                end: crate::time_utils::parse_time(word["end"].as_str().unwrap()),
                text: word["text"].as_str().unwrap_or_default().to_string(),
            })
            .collect();
        for intensity in [Intensity::Chill, Intensity::Punchy, Intensity::Hyper] {
            let kept =
                tighten_clips(Path::new(&source), &[vec![range]], &words, intensity, None).unwrap();
            let total: f64 = kept[0].iter().map(|(a, b)| b - a).sum();
            println!(
                "{intensity:?}: {:.1} s -> {total:.1} s ({:.0}% shorter), {} cuts",
                range.1 - range.0,
                100.0 * (1.0 - total / (range.1 - range.0)),
                kept[0].len() - 1
            );
            for (a, b) in &kept[0] {
                let text: Vec<&str> = words
                    .iter()
                    .filter(|w| {
                        let m = (w.start + w.end) / 2.0;
                        *a <= m && m < *b
                    })
                    .map(|w| w.text.as_str())
                    .collect();
                println!("   {a:7.2}-{b:7.2}  {}", text.join(" "));
            }
        }
    }
}
