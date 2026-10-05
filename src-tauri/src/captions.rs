//! Burned-in captions for vertical shorts (shorts phase S3).
//!
//! Short chunks of one to three words, the word being spoken highlighted,
//! white on a dark outline: the look viewers expect on Shorts, Reels and
//! TikTok, readable without sound.
//!
//! Captions are drawn here rather than by ffmpeg: the `ass`/`subtitles` and
//! `drawtext` filters need libass/freetype, which not every ffmpeg build has
//! (Homebrew's default doesn't). Each caption state becomes a transparent
//! PNG; an `ffconcat` list plays them as one timed image stream, laid over
//! the video with `overlay`, which every build has.

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use std::fmt::Write as _;
use std::path::Path;

/// Poppins ExtraBold (SIL OFL 1.1), google/fonts 7dc16b7d, SHA-256
/// f2ab17c1…f958e1. The licence ships next to it in `fonts/`.
static FONT: &[u8] = include_bytes!("../fonts/Poppins-ExtraBold.ttf");

/// A word on some timeline (seconds).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TimedWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptionStyle {
    /// Font size in output pixels.
    pub font_px: f32,
    pub max_words: usize,
    /// A chunk ends after this many characters...
    pub max_chars: usize,
    /// ...or at a pause longer than this (seconds).
    pub max_pause: f64,
    /// Vertical centre of the captions as a share of the frame height.
    pub center_y: f32,
    /// Lines may use this share of the frame width.
    pub max_width: f32,
    pub fill: [u8; 3],
    pub highlight: [u8; 3],
    /// Outline thickness as a share of the font size.
    pub stroke: f32,
    pub uppercase: bool,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        Self {
            font_px: 92.0,
            max_words: 3,
            max_chars: 18,
            max_pause: 0.45,
            // Below the faces (eye line at the upper third), clear of the
            // platforms' UI at the bottom.
            center_y: 0.68,
            max_width: 0.86,
            fill: [255, 255, 255],
            highlight: [255, 214, 10],
            stroke: 0.11,
            uppercase: false,
        }
    }
}

/// The clip's words on the output timeline: `ranges` (source seconds) play
/// back to back. Words are clipped to their range; words outside all ranges
/// are dropped.
pub(crate) fn output_words(words: &[TimedWord], ranges: &[(f64, f64)]) -> Vec<TimedWord> {
    let mut out = Vec::new();
    let mut offset = 0.0;
    for &(start, end) in ranges {
        for word in words {
            // Mostly inside: at least half the word is in the range.
            let overlap = word.end.min(end) - word.start.max(start);
            if overlap <= 0.0 || overlap < (word.end - word.start) / 2.0 {
                continue;
            }
            out.push(TimedWord {
                start: offset + word.start.max(start) - start,
                end: offset + word.end.min(end) - start,
                text: word.text.clone(),
            });
        }
        offset += end - start;
    }
    out
}

/// Words grouped into caption chunks.
pub(crate) fn chunk_words(words: &[TimedWord], style: &CaptionStyle) -> Vec<Vec<TimedWord>> {
    let mut chunks: Vec<Vec<TimedWord>> = Vec::new();
    let mut current: Vec<TimedWord> = Vec::new();
    for word in words {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }
        let chars: usize = current
            .iter()
            .map(|w| w.text.chars().count() + 1)
            .sum::<usize>()
            + text.chars().count();
        let pause = current.last().map_or(0.0, |last| word.start - last.end);
        if !current.is_empty()
            && (current.len() >= style.max_words
                || chars > style.max_chars
                || pause > style.max_pause)
        {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(TimedWord {
            text: text.to_string(),
            ..word.clone()
        });
        // Sentence and clause ends close a chunk.
        if text.ends_with(['.', '!', '?', ',', ';', ':']) {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// An RGBA image.
#[derive(Debug, Clone)]
pub(crate) struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Lays out and draws caption chunks into a fixed-size band.
pub(crate) struct CaptionPainter {
    font: FontRef<'static>,
    style: CaptionStyle,
    width: u32,
    /// Height of the band the captions are drawn into.
    pub height: u32,
}

/// A laid-out word: its glyphs' positions and which line it's on.
struct PlacedWord {
    glyphs: Vec<(ab_glyph::GlyphId, f32, f32)>,
}

impl CaptionPainter {
    pub(crate) fn new(style: CaptionStyle, frame_width: u32) -> Result<Self, String> {
        let font = FontRef::try_from_slice(FONT)
            .map_err(|e| format!("Failed to load the caption font: {e}"))?;
        let scaled = font.as_scaled(PxScale::from(style.font_px));
        let line = scaled.ascent() - scaled.descent() + scaled.line_gap();
        let margin = style.font_px * (style.stroke + 0.15);
        let height = (2.0 * line + 2.0 * margin).ceil() as u32 & !1;
        Ok(Self {
            font,
            style,
            width: frame_width,
            height,
        })
    }

    fn display(&self, text: &str) -> String {
        if self.style.uppercase {
            text.to_uppercase()
        } else {
            text.to_string()
        }
    }

    /// Glyph positions for `words`, centred, wrapped onto up to two lines.
    fn layout(&self, words: &[TimedWord]) -> Vec<PlacedWord> {
        let scaled = self.font.as_scaled(PxScale::from(self.style.font_px));
        let space = scaled.h_advance(self.font.glyph_id(' '));
        let advance = |text: &str| {
            let mut width = 0.0;
            let mut previous = None;
            for c in text.chars() {
                let id = self.font.glyph_id(c);
                if let Some(previous) = previous {
                    width += scaled.kern(previous, id);
                }
                width += scaled.h_advance(id);
                previous = Some(id);
            }
            width
        };
        let texts: Vec<String> = words.iter().map(|w| self.display(&w.text)).collect();
        let widths: Vec<f32> = texts.iter().map(|t| advance(t)).collect();

        // Greedy wrap into lines no wider than the limit.
        let limit = self.width as f32 * self.style.max_width;
        let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
        let mut line_width = 0.0;
        for (index, &width) in widths.iter().enumerate() {
            let line = lines.last_mut().expect("never empty");
            let added = if line.is_empty() {
                width
            } else {
                space + width
            };
            if !line.is_empty() && line_width + added > limit {
                lines.push(vec![index]);
                line_width = width;
            } else {
                line.push(index);
                line_width += added;
            }
        }

        let line_height = scaled.ascent() - scaled.descent() + scaled.line_gap();
        let block = line_height * lines.len() as f32;
        let top = (self.height as f32 - block) / 2.0;
        let mut placed: Vec<PlacedWord> = words
            .iter()
            .map(|_| PlacedWord { glyphs: Vec::new() })
            .collect();
        for (row, line) in lines.iter().enumerate() {
            let total: f32 = line.iter().map(|&i| widths[i]).sum::<f32>()
                + space * line.len().saturating_sub(1) as f32;
            let mut x = (self.width as f32 - total) / 2.0;
            let baseline = top + row as f32 * line_height + scaled.ascent();
            for &index in line {
                let mut previous = None;
                for c in texts[index].chars() {
                    let id = self.font.glyph_id(c);
                    if let Some(previous) = previous {
                        x += scaled.kern(previous, id);
                    }
                    placed[index].glyphs.push((id, x, baseline));
                    x += scaled.h_advance(id);
                    previous = Some(id);
                }
                x += space;
            }
        }
        placed
    }

    /// Coverage (0-1) per pixel, and which word each pixel belongs to.
    fn rasterize(&self, placed: &[PlacedWord]) -> (Vec<f32>, Vec<u16>) {
        let (w, h) = (self.width as i32, self.height as i32);
        let mut coverage = vec![0f32; (w * h) as usize];
        let mut owner = vec![u16::MAX; (w * h) as usize];
        for (index, word) in placed.iter().enumerate() {
            for &(id, x, baseline) in &word.glyphs {
                let glyph = id.with_scale_and_position(self.style.font_px, point(x, baseline));
                let Some(outlined) = self.font.outline_glyph(glyph) else {
                    continue;
                };
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, c| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if (0..w).contains(&px) && (0..h).contains(&py) {
                        let i = (py * w + px) as usize;
                        if c > coverage[i] {
                            coverage[i] = coverage[i].max(c);
                            owner[i] = index as u16;
                        }
                    }
                });
            }
        }
        (coverage, owner)
    }

    /// Grow `coverage` by a disc of `radius` pixels (anti-aliased edge).
    fn dilate(&self, coverage: &[f32], radius: f32) -> Vec<f32> {
        let (w, h) = (self.width as i32, self.height as i32);
        let reach = radius.ceil() as i32 + 1;
        let mut out = vec![0f32; coverage.len()];
        for y in 0..h {
            for x in 0..w {
                let c = coverage[(y * w + x) as usize];
                if c <= 0.0 {
                    continue;
                }
                for dy in -reach..=reach {
                    for dx in -reach..=reach {
                        let (px, py) = (x + dx, y + dy);
                        if !(0..w).contains(&px) || !(0..h).contains(&py) {
                            continue;
                        }
                        let distance = ((dx * dx + dy * dy) as f32).sqrt();
                        let edge = (radius + 0.5 - distance).clamp(0.0, 1.0) * c;
                        let i = (py * w + px) as usize;
                        if edge > out[i] {
                            out[i] = edge;
                        }
                    }
                }
            }
        }
        out
    }

    /// One frame per active word (and one with none active) for `chunk`.
    pub(crate) fn paint_chunk(&self, chunk: &[TimedWord]) -> Vec<Image> {
        let placed = self.layout(chunk);
        let (coverage, owner) = self.rasterize(&placed);
        let outline = self.dilate(&coverage, self.style.font_px * self.style.stroke);
        let shadow_offset = (self.style.font_px * 0.05).round() as i32;
        let (w, h) = (self.width as i32, self.height as i32);

        (0..chunk.len())
            .map(|active| {
                let mut data = vec![0u8; (w * h * 4) as usize];
                for y in 0..h {
                    for x in 0..w {
                        let i = (y * w + x) as usize;
                        // Soft shadow below-right, then the outline, then the fill.
                        let (sx, sy) = (x - shadow_offset, y - shadow_offset);
                        let shadow = if (0..w).contains(&sx) && (0..h).contains(&sy) {
                            outline[(sy * w + sx) as usize] * 0.45
                        } else {
                            0.0
                        };
                        let stroke = outline[i];
                        let fill = coverage[i];
                        let colour = if owner[i] as usize == active {
                            self.style.highlight
                        } else {
                            self.style.fill
                        };
                        // Composite over transparent: shadow, black stroke, fill.
                        let mut rgb = [0f32; 3];
                        let mut alpha = shadow;
                        let over = |rgb: &mut [f32; 3], alpha: &mut f32, c: [f32; 3], a: f32| {
                            let out = a + *alpha * (1.0 - a);
                            if out > 0.0 {
                                for k in 0..3 {
                                    rgb[k] = (c[k] * a + rgb[k] * *alpha * (1.0 - a)) / out;
                                }
                            }
                            *alpha = out;
                        };
                        over(&mut rgb, &mut alpha, [0.0; 3], stroke);
                        over(&mut rgb, &mut alpha, colour.map(f32::from), fill);
                        let o = i * 4;
                        data[o] = rgb[0].round() as u8;
                        data[o + 1] = rgb[1].round() as u8;
                        data[o + 2] = rgb[2].round() as u8;
                        data[o + 3] = (alpha * 255.0).round() as u8;
                    }
                }
                Image {
                    width: self.width,
                    height: self.height,
                    data,
                }
            })
            .collect()
    }

    pub(crate) fn blank(&self) -> Image {
        Image {
            width: self.width,
            height: self.height,
            data: vec![0; (self.width * self.height * 4) as usize],
        }
    }
}

fn write_png(path: &Path, image: &Image) -> Result<(), String> {
    let file = std::fs::File::create(path)
        .map_err(|e| format!("Failed to write '{}': {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&image.data))
        .map_err(|e| format!("Failed to encode '{}': {e}", path.display()))
}

/// A caption overlay ready for ffmpeg: an `ffconcat` list in `dir` and
/// where to put the band.
#[derive(Debug, Clone)]
pub(crate) struct CaptionOverlay {
    /// File name of the list, relative to the folder it was written to.
    pub list: String,
    /// Top of the caption band in the output frame.
    pub y: u32,
}

/// Write PNGs and an `ffconcat` list into `dir` showing `words` (output
/// timeline) over a clip of `duration` seconds and `frame` size.
pub(crate) fn write_overlay(
    dir: &Path,
    words: &[TimedWord],
    duration: f64,
    frame: (u32, u32),
    style: &CaptionStyle,
) -> Result<CaptionOverlay, String> {
    let painter = CaptionPainter::new(*style, frame.0)?;
    write_png(&dir.join("caption_blank.png"), &painter.blank())?;

    let mut list = String::from("ffconcat version 1.0\n");
    let mut time = 0.0;
    let entry = |list: &mut String, file: &str, until: f64, time: &mut f64| {
        let until = until.min(duration);
        if until - *time > 1e-3 {
            let _ = writeln!(list, "file {file}\nduration {:.4}", until - *time);
            *time = until;
        }
    };
    for (index, chunk) in chunk_words(words, style).iter().enumerate() {
        let frames = painter.paint_chunk(chunk);
        let chunk_start = chunk[0].start.max(time);
        entry(&mut list, "caption_blank.png", chunk_start, &mut time);
        for (active, image) in frames.iter().enumerate() {
            let name = format!("caption_{index:04}_{active}.png");
            write_png(&dir.join(&name), image)?;
            // Each word stays highlighted until the next one starts; the
            // last until the chunk ends.
            let until = chunk
                .get(active + 1)
                .map_or(chunk[active].end, |next| next.start);
            entry(&mut list, &name, until, &mut time);
        }
    }
    entry(&mut list, "caption_blank.png", duration, &mut time);
    // The concat demuxer ignores the last entry's duration; end on a blank.
    let _ = writeln!(list, "file caption_blank.png");
    std::fs::write(dir.join("captions.ffconcat"), list)
        .map_err(|e| format!("Failed to write the caption list: {e}"))?;

    let y = (frame.1 as f32 * style.center_y - painter.height as f32 / 2.0)
        .clamp(0.0, (frame.1 - painter.height) as f32) as u32
        & !1;
    Ok(CaptionOverlay {
        list: "captions.ffconcat".to_string(),
        y,
    })
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

    #[test]
    fn words_move_to_the_output_timeline() {
        let words = [
            word(10.0, 10.5, "one"),
            word(10.6, 11.0, "two"),
            word(20.0, 20.4, "three"),
            // Mostly outside the range: dropped.
            word(11.8, 12.6, "edge"),
            word(30.0, 30.5, "outside"),
        ];
        let out = output_words(&words, &[(20.0, 21.0), (10.0, 12.0)]);
        let expected = [
            word(0.0, 0.4, "three"),
            word(1.0, 1.5, "one"),
            word(1.6, 2.0, "two"),
        ];
        assert_eq!(out.len(), expected.len(), "{out:?}");
        for (got, want) in out.iter().zip(&expected) {
            assert_eq!(got.text, want.text);
            assert!((got.start - want.start).abs() < 1e-9, "{out:?}");
            assert!((got.end - want.end).abs() < 1e-9, "{out:?}");
        }
    }

    #[test]
    fn chunks_break_at_word_counts_punctuation_and_pauses() {
        let style = CaptionStyle::default();
        let words = [
            word(0.0, 0.2, "So"),
            word(0.2, 0.4, "this"),
            word(0.4, 0.6, "is"),
            word(0.6, 0.8, "where"),
            word(0.8, 1.0, "it"),
            word(1.0, 1.3, "starts."),
            word(1.3, 1.5, "Then"),
            // Long pause before the next word.
            word(2.5, 2.8, "silence"),
        ];
        let chunked = chunk_words(&words, &style);
        let chunks: Vec<Vec<&str>> = chunked
            .iter()
            .map(|chunk| chunk.iter().map(|w| w.text.as_str()).collect())
            .collect();
        assert_eq!(
            chunks,
            [
                vec!["So", "this", "is"],
                vec!["where", "it", "starts."],
                vec!["Then"],
                vec!["silence"]
            ]
        );
    }

    #[test]
    fn a_chunk_is_painted_once_per_active_word_with_the_highlight() {
        let style = CaptionStyle::default();
        let painter = CaptionPainter::new(style, 1080).unwrap();
        let frames = painter.paint_chunk(&[word(0.0, 0.5, "Hello"), word(0.5, 1.0, "world")]);
        assert_eq!(frames.len(), 2);
        let pixels = |image: &Image, colour: [u8; 3]| {
            image
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[3] == 255 && p[..3] == colour)
                .count()
        };
        // First frame: "Hello" highlighted, "world" white; then the reverse.
        let (yellow_0, white_0) = (
            pixels(&frames[0], style.highlight),
            pixels(&frames[0], style.fill),
        );
        let (yellow_1, white_1) = (
            pixels(&frames[1], style.highlight),
            pixels(&frames[1], style.fill),
        );
        assert!(yellow_0 > 1000 && white_0 > 1000, "{yellow_0} {white_0}");
        assert!(yellow_1 > 1000 && white_1 > 1000, "{yellow_1} {white_1}");
        // The text sits in the middle, with a dark outline around it.
        let image = &frames[0];
        let dark = image
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 200 && p[0] < 40 && p[1] < 40 && p[2] < 40)
            .count();
        assert!(dark > 1000, "outline pixels: {dark}");
        let corner = &image.data[..4];
        assert_eq!(corner[3], 0, "transparent outside the text");
    }

    #[test]
    fn the_overlay_list_times_every_state_and_ends_blank() {
        let dir = tempfile::tempdir().unwrap();
        let words = [word(0.5, 0.9, "Hello"), word(1.0, 1.4, "world.")];
        let overlay = write_overlay(
            dir.path(),
            &words,
            3.0,
            (1080, 1920),
            &CaptionStyle::default(),
        )
        .unwrap();
        let list = std::fs::read_to_string(dir.path().join(&overlay.list)).unwrap();
        assert_eq!(
            list,
            "ffconcat version 1.0\n\
             file caption_blank.png\nduration 0.5000\n\
             file caption_0000_0.png\nduration 0.5000\n\
             file caption_0000_1.png\nduration 0.4000\n\
             file caption_blank.png\nduration 1.6000\n\
             file caption_blank.png\n"
        );
        assert!(dir.path().join("caption_0000_1.png").is_file());
        assert!(
            overlay.y > 1920 / 2 && overlay.y < 1920 * 3 / 4,
            "{}",
            overlay.y
        );
    }
}
