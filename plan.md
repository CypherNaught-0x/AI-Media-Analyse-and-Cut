# **App Summary**

## **Media AI Cutter** is a cross-platform desktop application built with Tauri v2 (Rust) and Vue 3 (TypeScript/pnpm). It serves as a local media lab that allows users to drag and drop video or audio files for intelligent processing.

  * **Core Function**: Automatically extracts audio, optimizes it for API usage (AAC/OGG conversion), and generates highly accurate transcripts using Google's **Gemini 2.5 Flash/Pro** models.
  * **Key Features**:
      * **Auto-setup**: Automatically downloads the required FFmpeg binaries on the first run.
      * **Smart Uploads**: Uses the Google Files API for large media to bypass token limits.
      * **AI Editing**: Users can edit transcripts to cut video segments non-linearly.
      * **Viral Shorts**: AI-driven detection of "interesting" segments for social media generation.
      * **Local Privacy Option**: (Planned) Support for offline transcription via NVIDIA Parakeet.

### **Unit Test Requirements**

To ensure reliability, the following test coverage is required:

  * **Audio Conversion**: Assert that `ffmpeg-sidecar` correctly converts a sample `.mp4` to `.aac` and returns the correct file path.
  * **Size Calculation**: Test the file size estimator function against known byte counts to determine if the Files API is needed (threshold \> 20MB).
  * **JSON Parsing**: Mock a Gemini response (with and without timestamps) and assert that the Rust struct correctly parses it into `Segment` objects.
  * **PromptBuilder**: Assert that the system prompt correctly injects the user's glossary and context strings.

-----

### **Implementation Steps**

#### **Step 1: Project Initialization & Prerequisites**

**Requirements**: `pnpm`, `TypeScript`, `Tauri v2 CLI`.

1.  **Setup Environment**:
      * Install pnpm: `npm install -g pnpm`
      * Ensure Rust is up to date: `rustup update`
2.  **Scaffold Project**:
    ```bash
    pnpm create tauri-app@latest --template vue-ts
    # Select "TypeScript" and "pnpm" explicitly
    ```
3.  **Install Frontend Dependencies**:
    ```bash
    cd media-ai-cutter
    pnpm install
    pnpm add -D tailwindcss postcss autoprefixer
    pnpm tailwindcss init -p
    ```
      * *Config*: Ensure `tsconfig.json` has `"strict": true`.

#### **Step 2: FFmpeg Sidecar & Auto-Download**

**Goal**: Remove manual binary management and use the crate's auto-download feature.

1.  **Add Dependency**:
    In `src-tauri/Cargo.toml`:
    ```toml
    [dependencies]
    ffmpeg-sidecar = "2.0" # Check latest version
    anyhow = "1.0"
    ```
2.  **Implement Auto-Download**:
    In `src-tauri/src/main.rs`, adds a startup check.
      * **Logic**: On app launch, call `ffmpeg_sidecar::download::auto_download().unwrap()`.
      * **UX**: Show a "Initializing AI Engine..." splash screen on the frontend while this runs (it downloads \~70MB).
3.  **Refine Configuration**:
      * Documentation notes that `auto_download` places binaries in a local directory. Ensure your Tauri permissions (in `tauri.conf.json`) allow executing binaries from the app data folder if strictly sandboxed, though `ffmpeg-sidecar` usually handles the path resolution internally.

#### **Step 3: Audio Pre-processing (The Pipeline)**

**Goal**: Convert audio to a bandwidth-efficient format (`audio/ogg` or `audio/aac`) before sending to AI.

1.  **Conversion Command**:
    Create a Rust function `prepare_audio_for_ai(input: &str) -> Result<PathBuf>`.
      * Use `ffmpeg-sidecar` to run:
        `ffmpeg -i input.mp4 -vn -c:a libvorbis -q:a 4 output.ogg` (or `-c:a aac -b:a 32k` for extreme compression).
      * *Why*: OGG/Vorbis is highly efficient for speech and natively supported by Gemini.
2.  **Size Calculation**:
      * Implement `fs::metadata(&output_path)?.len()` to get bytes.
      * **Decision Logic**:
          * If `< 18MB`: Use Inline Data (base64 encoded in `litellm_rs` payload).
          * If `> 18MB`: Use Google Files API (Step 4).

#### **Step 4: Google Files API Integration**

**Goal**: Handle large files that exceed the standard prompt payload limit.

  * *Note*: `litellm_rs` focuses on the completion/generation interface. For the **Files API** (uploading the blob), we need a standard HTTP client.

<!-- end list -->

1.  **Add `reqwest`**: Use `reqwest = { version = "0.11", features = ["json", "multipart"] }` purely for the upload step.
2.  **Upload Logic**:
      * POST to `https://generativelanguage.googleapis.com/upload/v1beta/files?key=YOUR_API_KEY`.
      * Get `file_uri` from the response (e.g., `https://generativelanguage.googleapis.com/v1beta/files/abc-123`).
      * **State**: Verify the file state is `ACTIVE` before proceeding.

#### **Step 5: Intelligence Layer (litellm\_rs)**

**Goal**: Use `litellm_rs` to interact with Gemini 2.5.

1.  **Add Dependency**:
    ```toml
    [dependencies]
    litellm-rs = "0.1" # Verify latest version
    tokio = { version = "1", features = ["full"] }
    ```
2.  **Client Configuration**:
      * Model: `gemini/gemini-2.5-flash` or `gemini/gemini-2.5-pro` (Check specific provider string in `litellm_rs` docs, usually `gemini/` prefix routes to Google).
3.  **Construct the Payload**:
      * If **Inline** (Small file):
        Pass the audio as a base64 string in the content block (if `litellm_rs` supports multimodal content blocks), or fallback to `reqwest` if the crate is strictly text-only.
      * If **Files API** (Large file):
        Pass the `file_uri` obtained in Step 4.
4.  **The Prompt**:
      * **System Prompt**:
        > "You are a professional video editor assistant. Your task is to transcribe the audio and identify logical segments."
      * **User Prompt**:
        > "Analyze the following audio.
        > Context: {{ user\_context }}
        > Glossary: {{ glossary\_terms }}
        > [WISH FOR TIMESTAMPS]: Please output the transcription in a strict JSON format with 'start', 'end', 'speaker', and 'text' fields. Ensure timestamps are in 'MM:SS' format.
        > *Note: This prompt is exemplary; the model may hallucinate timestamp formats without few-shot examples. Please verify output.*"

#### **Step 6: Frontend & Parsing (Vue + TS)**

1.  **Strict Types**:
    Define interfaces in `src/types/index.ts`:
    ```typescript
    interface TranscriptSegment {
      start: string;
      end: string;
      text: string;
      speaker: string;
    }
    ```
2.  **Parsing Rust Response**:
      * The `litellm_rs` response will likely be a string containing JSON code blocks (markdown).
      * Implement a regex parser in Rust or TS to extract the JSON array from the Markdown code fences.
3.  **Editor UI**:
      * Render the segments.
      * Clicking a segment uses the HTML5 Video API (`video.currentTime = ...`) to jump to the specific second.

#### **Step 7: Video Generation (Cutting)**

1.  **Execution**:
      * Receive the list of "kept" segments from the UI.
      * Use `ffmpeg-sidecar` to run the cut command.
      * *Optimization*: Use a `filter_complex` command to cut and concat in a single pass to avoid intermediate files if possible.