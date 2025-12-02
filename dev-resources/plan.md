# AI Media Cutter - Development Plan

## Project Overview

A desktop application for intelligent video and audio segmentation using AI. Supports transcription with timestamps and speaker annotations, segment identification, and intelligent video cutting for both full segments and social media clips.

## Technology Stack

- **Frontend**: Vue.js 3 + TypeScript + TailwindCSS
- **Backend**: Rust + Tauri
- **AI**: LiteLLM (Google Gemini, OpenAI-compatible APIs), Future: Local ONNX models
- **Media Processing**: FFmpeg

---

## Feature Checklist

### udio Extraction

- [x] Extract audio track from video files using FFmpeg
- [x] Convert to OGG format (libvorbis) for AI processing
- [x] Monitor file size for upload decisions
- [x] Support direct audio file input (skip extraction)

### LLM Configuration & Settings

- [x] Settings page for API configuration
- [x] Support for multiple LLM providers (Google Gemini, OpenAI, LiteLLM)
- [x] Base URL normalization (strip trailing slashes, add https://)
- [x] Model selection with fetch from API
- [x] Manual model input fallback
- [x] API key management (stored in localStorage)
- [x] Proper authentication (query parameter for Google, Bearer token for OpenAI)

### Transcription Features

#### Current Implementation

- [x] Google Gemini API integration with file upload (>20MB)
- [x] OpenAI-compatible API support with base64 encoding
- [x] Base64 file encoding via Tauri command
- [x] Context input field for content description
- [x] Glossary input field for keyword descriptions
- [x] Basic transcript parsing with timestamps

#### Missing Features

- [ ] **Speaker count configuration input**
  - Add UI field for number of speakers
  - Pass to system prompt for better speaker annotation
- [ ] **Enhanced system prompt with speaker information**
  - Include speaker count in prompt
  - Request clear speaker labels (e.g., Speaker 1, Speaker 2)
- [ ] **Improved JSON parsing and validation**
  - Validate timestamp format (MM:SS)
  - Handle malformed responses gracefully
  - Show parsing errors to user
- [ ] **Error handling and retry logic**
  - Retry failed API calls
  - Better error messages
  - Timeout handling

#### Future: Local Transcription Option

- [ ] **Integrate nvidia/parakeet-tdt-0.6b-v3 (ONNX)**
  - Research ONNX integration in Rust/Tauri
  - Implement model loading and inference
  - Support for multi-language transcription
  - Word-level and segment-level timestamps
  - Handle long audio (up to 24 min with full attention, 3hr with local attention)
- [ ] **Audio segmentation for local transcription**
  - Split audio into chunks when approaching limits
  - Merge transcripts from multiple chunks
  - Preserve timestamp continuity across chunks
- [ ] **Local vs Cloud toggle in Settings**
  - UI option to choose transcription method
  - Auto-fallback if local model unavailable

### Segment Identification & Editing

#### Current Implementation

- [x] Basic transcript parsing into segments
- [x] Display segments in UI with timestamps, speaker, and text
- [x] Segment list visualization

#### Missing Features

- [ ] **AI-powered segment identification**
  - Analyze transcript for topic changes
  - Detect natural break points
  - Group related content together
  - Configurable segment rules (min/max duration)
- [ ] **Manual segment editing**
  - Click to edit segment start/end times
  - Drag-and-drop timeline editor
  - Merge adjacent segments
  - Split segments at cursor position
  - Delete/add segments
- [ ] **Segment preview**
  - Play segment in embedded player
  - Visual waveform/timeline
  - Keyboard shortcuts (Space: play/pause, Arrow keys: navigate)
- [ ] **Segment metadata**
  - Assign titles/names to segments
  - Add notes/descriptions
  - Tag segments (e.g., intro, main content, outro)

### Video Cutting

#### Current Implementation

- [x] Basic video cutting by segments using FFmpeg
- [x] Output path generation (\_cut suffix)

#### Missing Features

- [ ] **Progress indication**
  - Real-time progress bar for cutting operations
  - Estimated time remaining
  - Cancel operation button
- [ ] **Batch export**
  - Export all segments at once
  - Choose output directory
  - Naming convention (e.g., segment_001.mp4, segment_002.mp4)
- [ ] **Output configuration**
  - Format selection (MP4, MKV, AVI, etc.)
  - Quality/bitrate settings
  - Resolution options (keep original, or resize)
  - Codec selection
- [ ] **Advanced cutting options**
  - Include/exclude specific segments
  - Add fade in/out effects
  - Add intro/outro clips
  - Preserve or re-encode (fast vs quality)

### Short Clips Generation (NEW)

- [ ] **Interestingness scoring algorithm**
  - Analyze segments for engagement factors:
    - Speaker changes (dialogue vs monologue)
    - Keyword density from glossary
    - Length appropriateness (15-60s ideal)
    - Audio energy/volume variations
  - Score each segment 0-100
- [ ] **Configurable clip count**
  - UI input for number of clips to generate (x)
  - Default to 3-5 clips
- [ ] **Duration constraints**
  - Platform presets: LinkedIn (30-60s), Instagram (15-60s), TikTok (15-60s)
  - Custom duration ranges
  - Auto-trim or extend segments to fit
- [ ] **Auto-selection of best segments**
  - Sort segments by interestingness score
  - Select top x segments
  - Avoid overlapping clips
  - Ensure variety (different speakers, topics)
- [ ] **Platform-specific optimization**
  - Aspect ratio conversion (16:9, 9:16, 1:1)
  - Target resolution (1080p, 720p)
  - Bitrate optimization for platform
  - Add captions/subtitles overlay
- [ ] **Thumbnail generation**
  - Extract frame from middle of clip
  - Add text overlay with title
  - Save as separate image files
- [ ] **Clip preview and approval**
  - Show selected clips before export
  - Allow manual adjustments
  - Regenerate if unsatisfied

### Audio File Support

- [ ] **Direct audio file processing**
  - Skip video extraction for audio files (MP3, WAV, OGG, etc.)
  - All transcription features work on audio
- [ ] **Audio-specific features**
  - Waveform visualization
  - Audio-only export options
  - Podcast segment detection

---

## Current Status

### Completed

- FFmpeg integration and auto-download
- Audio extraction from video files
- Settings page with multi-provider LLM configuration
- Google Gemini API integration (with file upload for large files)
- OpenAI/LiteLLM compatibility (with base64 encoding)
- Base64 file encoding via Tauri command
- Basic transcription with context and glossary fields
- Segment display in UI
- Basic video cutting

### In Progress

- Improving transcription accuracy and error handling
- UI/UX refinements

### Planned (High Priority)

1. Speaker count configuration
2. Enhanced segment editing (manual adjustments)
3. Short clips generation with AI scoring
4. Progress indicators for all long operations
5. Better error handling and user feedback

### Future Considerations

- Local transcription with nvidia/parakeet-tdt-0.6b-v3 (ONNX)
- Batch processing multiple files
- Export templates for different platforms
- Cloud storage integration for outputs
- Collaboration features (share segments, get feedback)

---

## Technical Implementation Notes

### Audio Processing

- Uses `ffmpeg-sidecar` with auto-download
- Converts to OGG format (libvorbis codec, quality setting: q:a 4)
- Tauri command: `prepare_audio_for_ai`

### API Integration

- Dynamic endpoint detection (Google vs OpenAI-compatible)
- Conditional authentication:
  - Google: API key as query parameter (`?key=`)
  - OpenAI/LiteLLM: Bearer token in `Authorization` header
- File handling:
  - Google: File upload API for files >20MB
  - Others: Base64 encoding via `read_file_as_base64` command
- Different request formats:
  - Google: `contents`, `system_instruction`
  - OpenAI: `model`, `messages` with roles

### Frontend Architecture

- Vue 3 Composition API with TypeScript
- Reactive settings management (`useSettings` composable with localStorage)
- Tauri commands for all backend operations
- Modern gradient-based UI with dark mode theme
- TailwindCSS for styling

### Backend Architecture

- Rust with Tauri for native desktop app
- Modular structure:
  - `gemini.rs`: LLM client with multi-provider support
  - `upload.rs`: Google Files API upload (conditional)
  - `video.rs`: FFmpeg-based video cutting
  - `lib.rs`: Tauri commands and app initialization

---

## Next Steps (Prioritized)

1. **Add speaker count input** - Quick win for better transcription
2. **Implement segment editing UI** - Core functionality for user control
3. **Short clips generation MVP** - High-value feature for social media users
4. **Progress indicators** - Better UX for long operations
5. **Research local transcription** - Evaluate ONNX integration feasibility
6. **Batch processing** - Allow processing multiple files in queue
7. **Platform export presets** - Simplify output for different social platforms
