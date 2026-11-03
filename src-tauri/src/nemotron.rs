//! NVIDIA Nemotron 3.5 Transcribe 30B-A4B, on device.
//!
//! A speech-LLM: a speech encoder feeds a 30B mixture-of-experts language
//! model (about 4B active per token) that writes the transcript, with
//! punctuation, in ~40 languages, and follows an instruction, which is how the
//! project glossary reaches it ("Context terms: ..."). The encoder file's CTC
//! head times every word and its speaker branch labels speakers.
//!
//! It does not run on the ONNX runtime the Parakeet engine uses. It runs on
//! NVIDIA's NeMo-Speech.cpp (llama.cpp underneath), whose release archives the
//! app downloads, pinned by version and SHA-256, next to the GGUF files from
//! `AtomicChat/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF`. The CLI is driven
//! out of process: `nemo-speech transcribe <wav> --format json`.
//!
//! The decoder is large (13.6-35 GB), so the build is chosen from the
//! machine's memory ([`recommend`]) and nothing is downloaded until the user
//! asks for it in Settings or the setup wizard.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::device::{self, DeviceInfo};
use crate::error::AppError;
use crate::local_asr::{
    build_transcript_segments, diarize, emit_progress, load_audio_16k_mono,
    resolve_sortformer_file, speaker_label_for_word, write_wav_16k_mono, WordWithSpeaker,
};
use crate::model_download::{ensure_verified_download, VerifiedDownload, HUGGING_FACE};
use crate::run_control::{run_blocking, RunControl};
use crate::video::TranscriptSegment;

const LABEL: &str = "Nemotron";
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

const MODEL_REPO: &str = "AtomicChat/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-GGUF";
/// The repository commit the files were checked against (2026-11-02).
const MODEL_REVISION: &str = "dbd06c5cad7a2f33c8e06be2fb3ef814dbd9aa59";
const MODEL_PREFIX: &str = "NVIDIA-Nemotron-3.5-Transcribe-30B-A4B";
const MODEL_SUBDIRECTORY: &str = "nemotron-transcribe";

const RUNTIME_VERSION: &str = "0.3.0";
const RUNTIME_RELEASES: &str = "https://github.com/NVIDIA/NeMo-Speech.cpp/releases/download";

/// A request takes at most one hour; chunks stay well below that.
const MAX_CHUNK_SECONDS: f64 = 50.0 * 60.0;
/// Room for the KV cache, encoder output and the rest of the process, on top
/// of the model files.
const RUNTIME_HEADROOM_GB: f64 = 1.5;

struct ModelFile {
    suffix: &'static str,
    sha256: &'static str,
    size: u64,
}

const ENCODER: ModelFile = ModelFile {
    suffix: "Encoder-F16",
    sha256: "4221029c8a565c4a80c507ffa760c716e595acf663f9f4273ce69910050e7ba2",
    size: 1_670_410_080,
};

/// Decoder precisions. Every one from Q2_0_L up stays within ±0.2 WER points
/// of BF16 on English, German, French and Spanish (model card); Q2_0_M adds
/// ~1.5 points of Chinese character error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum NemotronVariant {
    #[serde(rename = "Q8_0")]
    Q8,
    #[serde(rename = "Q5_0_XL")]
    Q5Xl,
    #[serde(rename = "IQ4_NL_L")]
    Iq4NlL,
    #[serde(rename = "IQ4_NL")]
    Iq4Nl,
    #[serde(rename = "Q2_0_L")]
    Q2L,
    #[serde(rename = "Q2_0_M")]
    Q2M,
}

/// Largest first.
const VARIANTS: [NemotronVariant; 6] = [
    NemotronVariant::Q8,
    NemotronVariant::Q5Xl,
    NemotronVariant::Iq4NlL,
    NemotronVariant::Iq4Nl,
    NemotronVariant::Q2L,
    NemotronVariant::Q2M,
];

impl NemotronVariant {
    fn file(self) -> ModelFile {
        match self {
            Self::Q8 => ModelFile {
                suffix: "Q8_0",
                sha256: "3d99075b58f15153d87e8922f54e12dda818bbb505c2e0d7d50cd4f30beac9f2",
                size: 35_004_643_232,
            },
            Self::Q5Xl => ModelFile {
                suffix: "AD-Q5_0_XL",
                sha256: "a1eef76726d252509032a84339f8300584d7590014a63139d2d008ff981e2f4f",
                size: 29_257_398_432,
            },
            Self::Iq4NlL => ModelFile {
                suffix: "AD-IQ4_NL_L",
                sha256: "20a883d2d3183aa77812c1b357fdef28fc3a23ba4cca370bc00e0a03875a10a0",
                size: 22_552_279_200,
            },
            Self::Iq4Nl => ModelFile {
                suffix: "AD-IQ4_NL",
                sha256: "0c47d4ab6c783e7606ec03714b354fab0b2052027a7ea1991e7e2d3009c345ad",
                size: 19_678_656_672,
            },
            Self::Q2L => ModelFile {
                suffix: "AD-Q2_0_L",
                sha256: "67eee66a7bb205867e3f9b7c0944e287fc4d2661db2280030e1c26e6ebdea7b4",
                size: 15_547_824_288,
            },
            Self::Q2M => ModelFile {
                suffix: "AD-Q2_0_M",
                sha256: "764dc3aedbc800a96346e188d74f56fc4d0932299e4771df1bc2e1c952ceed81",
                size: 13_572_208_800,
            },
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Q8 => "Q8_0 (closest to the original)",
            Self::Q5Xl => "Q5_0_XL",
            Self::Iq4NlL => "IQ4_NL_L",
            Self::Iq4Nl => "IQ4_NL (recommended)",
            Self::Q2L => "Q2_0_L",
            Self::Q2M => "Q2_0_M (for 16 GB GPUs)",
        }
    }

    /// Memory the model needs to load and run, in GB.
    fn required_gb(self) -> f64 {
        (self.file().size + ENCODER.size) as f64 / GIB + RUNTIME_HEADROOM_GB
    }
}

fn model_file_name(file: &ModelFile) -> String {
    format!("{MODEL_PREFIX}-{}.gguf", file.suffix)
}

fn model_download<'a>(file: &ModelFile, name: &'a str) -> VerifiedDownload<'a> {
    VerifiedDownload {
        url: format!("{HUGGING_FACE}/{MODEL_REPO}/resolve/{MODEL_REVISION}/{name}?download=1"),
        label: name,
        sha256: file.sha256,
        size: file.size,
    }
}

/// A NeMo-Speech.cpp release archive for one platform and backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeArchive {
    /// `macos-aarch64-metal`, ...
    id: &'static str,
    sha256: &'static str,
    size: u64,
    zip: bool,
}

const RUNTIMES: [RuntimeArchive; 7] = [
    RuntimeArchive {
        id: "macos-aarch64-metal",
        sha256: "2b972ed8b5dba07492212acf8cb75cbf34ecb141198ffa1997febe84b7c4d9ed",
        size: 7_239_335,
        zip: false,
    },
    RuntimeArchive {
        id: "macos-x86_64-cpu",
        sha256: "97e98aa5eb82dffa4f513f26336b5301d792c9fb43f720ac8f9520a0e92ffe3f",
        size: 7_742_012,
        zip: false,
    },
    RuntimeArchive {
        id: "linux-x86_64-cuda",
        sha256: "eca00a1f67d3ca200ddba11dd71c4f3d67e044cc0939d17d073978e40b995917",
        size: 703_984_313,
        zip: false,
    },
    RuntimeArchive {
        id: "linux-x86_64-cpu",
        sha256: "255f12b722ab15b2a38a51cf125d56e3fa97058b528f7e895b39d6a473deb06e",
        size: 9_605_101,
        zip: false,
    },
    RuntimeArchive {
        id: "linux-aarch64-cpu",
        sha256: "56858eaf50d19b05902c043f65d0057352bdc107ae380c41053bfbddfc204bf1",
        size: 9_064_856,
        zip: false,
    },
    RuntimeArchive {
        id: "windows-x86_64-cuda",
        sha256: "d0d32ae1bf6ee0534767a47b2aa0e9e2ff6fe39643465c0baae9301d04f6ed42",
        size: 508_625_181,
        zip: true,
    },
    RuntimeArchive {
        id: "windows-x86_64-cpu",
        sha256: "4ed9740e361e4653f56c3dce7d1e81883a1c8b831c1bd7d01567fff1130b5b05",
        size: 8_694_509,
        zip: true,
    },
];

impl RuntimeArchive {
    fn file_name(&self) -> String {
        let extension = if self.zip { "zip" } else { "tar.gz" };
        format!("nemo-speech-{RUNTIME_VERSION}-{}.{extension}", self.id)
    }

    fn url(&self) -> String {
        format!("{RUNTIME_RELEASES}/v{RUNTIME_VERSION}/{}", self.file_name())
    }

    fn directory_name(&self) -> String {
        format!("nemo-speech-{RUNTIME_VERSION}-{}", self.id)
    }
}

/// Where inference runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum RunDevice {
    Metal,
    Cuda,
    Cpu,
}

impl RunDevice {
    fn flag(self) -> &'static str {
        match self {
            Self::Metal => "metal",
            Self::Cuda => "cuda",
            Self::Cpu => "cpu",
        }
    }
}

/// How well Nemotron runs on a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum FitRating {
    /// On the GPU with the recommended precision.
    Good,
    /// On the GPU, but only a smaller precision fits and memory is tight.
    Tight,
    /// On the CPU: works, but much slower than real time on long files.
    Slow,
    /// Not enough memory, or no runtime for this platform.
    Unsupported,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NemotronFit {
    pub rating: FitRating,
    pub variant: Option<NemotronVariant>,
    pub device: RunDevice,
    pub message: String,
}

/// The runtime archive for this platform. CUDA only when an NVIDIA GPU is
/// present; everything else uses the CPU build (Metal on Apple Silicon).
fn runtime_for(os: &str, arch: &str, has_nvidia: bool) -> Option<RuntimeArchive> {
    let id = match (os, arch, has_nvidia) {
        ("macos", "aarch64", _) => "macos-aarch64-metal",
        ("macos", "x86_64", _) => "macos-x86_64-cpu",
        ("linux", "x86_64", true) => "linux-x86_64-cuda",
        ("linux", "x86_64", false) => "linux-x86_64-cpu",
        ("linux", "aarch64", _) => "linux-aarch64-cpu",
        ("windows", "x86_64", true) => "windows-x86_64-cuda",
        ("windows", "x86_64", false) => "windows-x86_64-cpu",
        _ => return None,
    };
    RUNTIMES.iter().copied().find(|runtime| runtime.id == id)
}

fn runtime_for_device(device: &DeviceInfo) -> Option<RuntimeArchive> {
    runtime_for(
        &device.os,
        &device.arch,
        device.nvidia_memory_gb().is_some(),
    )
}

/// The largest precision a memory budget holds, preferring IQ4_NL: the larger
/// ones gain under 0.2 WER points for a third more download and slower
/// decoding (it is memory-bandwidth bound), so they stay an explicit choice.
fn variant_for_budget(budget_gb: f64) -> Option<NemotronVariant> {
    [
        NemotronVariant::Iq4Nl,
        NemotronVariant::Q2L,
        NemotronVariant::Q2M,
    ]
    .into_iter()
    .find(|variant| variant.required_gb() <= budget_gb)
}

/// Which build to use on `device`, and how well it will run.
pub(crate) fn recommend(device: &DeviceInfo) -> NemotronFit {
    let unsupported = |message: String| NemotronFit {
        rating: FitRating::Unsupported,
        variant: None,
        device: RunDevice::Cpu,
        message,
    };
    if runtime_for_device(device).is_none() {
        return unsupported(format!(
            "NeMo-Speech.cpp has no build for {} {}.",
            device.os, device.arch
        ));
    }
    let smallest = NemotronVariant::Q2M.required_gb();

    if device.apple_silicon {
        // Metal's working-set limit: `nemo-speech doctor` reports 28.1 GiB
        // of a 36 GB M3 Pro (78%); three quarters leaves a little margin.
        let budget = device.total_memory_gb * 0.75;
        return match variant_for_budget(budget) {
            Some(variant @ NemotronVariant::Iq4Nl) => NemotronFit {
                rating: FitRating::Good,
                variant: Some(variant),
                device: RunDevice::Metal,
                message: format!(
                    "Runs on the GPU with the recommended {} build.",
                    variant_short(variant)
                ),
            },
            Some(variant) => NemotronFit {
                rating: FitRating::Tight,
                variant: Some(variant),
                device: RunDevice::Metal,
                message: format!(
                    "Fits with the smaller {} build. Close other large apps while transcribing.",
                    variant_short(variant)
                ),
            },
            None => unsupported(format!(
                "Needs a Mac with at least 24 GB of memory; this one has {:.0} GB.",
                device.total_memory_gb
            )),
        };
    }

    if let Some(vram) = device.nvidia_memory_gb() {
        match variant_for_budget(vram) {
            Some(variant @ NemotronVariant::Iq4Nl) => {
                return NemotronFit {
                    rating: FitRating::Good,
                    variant: Some(variant),
                    device: RunDevice::Cuda,
                    message: format!(
                        "Runs on the NVIDIA GPU ({vram:.0} GB) with the recommended {} build.",
                        variant_short(variant)
                    ),
                }
            }
            Some(variant) => {
                return NemotronFit {
                    rating: FitRating::Tight,
                    variant: Some(variant),
                    device: RunDevice::Cuda,
                    message: format!(
                        "Fits the NVIDIA GPU ({vram:.0} GB) with the smaller {} build.",
                        variant_short(variant)
                    ),
                }
            }
            None => {} // Too small: fall through to the CPU.
        }
    }

    // CPU: needs the model in system RAM next to everything else.
    match variant_for_budget(device.total_memory_gb - 4.0) {
        Some(variant) => NemotronFit {
            rating: FitRating::Slow,
            variant: Some(variant),
            device: RunDevice::Cpu,
            message: format!(
                "No GPU it can use: it would run on the CPU ({} build), which is slow for long recordings.",
                variant_short(variant)
            ),
        },
        None => unsupported(format!(
            "Needs at least {:.0} GB of GPU memory or {:.0} GB of RAM; this machine has {:.0} GB of RAM.",
            smallest,
            smallest + 4.0,
            device.total_memory_gb
        )),
    }
}

fn variant_short(variant: NemotronVariant) -> &'static str {
    variant.file().suffix.trim_start_matches("AD-")
}

// --- Paths -----------------------------------------------------------------

fn app_data(app: &tauri::AppHandle) -> Result<PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|error| anyhow!(error.to_string()))
}

fn models_dir(app: &tauri::AppHandle) -> Result<PathBuf> {
    Ok(app_data(app)?.join("models").join(MODEL_SUBDIRECTORY))
}

fn runtime_dir(app: &tauri::AppHandle, runtime: &RuntimeArchive) -> Result<PathBuf> {
    Ok(app_data(app)?
        .join("runtimes")
        .join(runtime.directory_name()))
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "nemo-speech.exe"
    } else {
        "nemo-speech"
    }
}

/// The CLI inside an extracted archive (its layout is `<name>/bin/...`).
fn find_binary(root: &Path, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut directories = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        } else if path.file_name().is_some_and(|name| name == binary_name()) {
            return Some(path);
        }
    }
    if depth == 0 {
        return None;
    }
    directories
        .into_iter()
        .find_map(|directory| find_binary(&directory, depth - 1))
}

fn installed_binary(app: &tauri::AppHandle, runtime: &RuntimeArchive) -> Option<PathBuf> {
    find_binary(&runtime_dir(app, runtime).ok()?, 3)
}

fn installed_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|meta| meta.len())
}

fn is_installed(directory: &Path, file: &ModelFile) -> bool {
    installed_size(&directory.join(model_file_name(file))) == Some(file.size)
}

// --- Status ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NemotronVariantInfo {
    pub variant: NemotronVariant,
    pub label: String,
    /// Decoder plus the shared encoder.
    #[specta(type = specta_typescript::Number)]
    pub download_bytes: u64,
    #[specta(type = specta_typescript::Number)]
    pub required_memory_gb: f64,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NemotronStatus {
    /// NeMo-Speech.cpp publishes a build for this platform.
    pub platform_supported: bool,
    pub runtime_installed: bool,
    /// Bytes still to download for the runtime (0 once installed).
    #[specta(type = specta_typescript::Number)]
    pub runtime_download_bytes: u64,
    pub encoder_installed: bool,
    pub variants: Vec<NemotronVariantInfo>,
    pub recommendation: NemotronFit,
    pub device: DeviceInfo,
    /// A download is running.
    pub installing: bool,
}

fn status(app: &tauri::AppHandle) -> Result<NemotronStatus> {
    let device = device::detect(&app_data(app)?.join("models"));
    let recommendation = recommend(&device);
    let runtime = runtime_for_device(&device);
    let models = models_dir(app)?;
    let encoder_installed = is_installed(&models, &ENCODER);
    let runtime_installed =
        runtime.is_some_and(|runtime| installed_binary(app, &runtime).is_some());
    Ok(NemotronStatus {
        platform_supported: runtime.is_some(),
        runtime_installed,
        runtime_download_bytes: match runtime {
            Some(runtime) if !runtime_installed => runtime.size,
            _ => 0,
        },
        encoder_installed,
        variants: VARIANTS
            .iter()
            .map(|&variant| NemotronVariantInfo {
                variant,
                label: variant.label().to_string(),
                download_bytes: variant.file().size + ENCODER.size,
                required_memory_gb: variant.required_gb(),
                installed: encoder_installed && is_installed(&models, &variant.file()),
            })
            .collect(),
        recommendation,
        device,
        installing: INSTALL.lock().expect("install lock poisoned").is_some(),
    })
}

/// What is installed, and what this machine should use.
#[tauri::command]
#[specta::specta]
pub async fn nemotron_status(app: tauri::AppHandle) -> Result<NemotronStatus, AppError> {
    Ok(tokio::task::spawn_blocking(move || status(&app))
        .await
        .map_err(|error| anyhow!(error.to_string()))??)
}

// --- Install ---------------------------------------------------------------

/// The running download, so it can be cancelled.
static INSTALL: Mutex<Option<tokio::task::AbortHandle>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallProgress {
    message: String,
    downloaded_bytes: u64,
    total_bytes: u64,
}

const INSTALL_EVENT: &str = "nemotron-install-progress";

/// Extract a release archive into `destination` (created fresh).
fn extract_archive(archive: &Path, destination: &Path, zip: bool) -> Result<()> {
    if destination.exists() {
        std::fs::remove_dir_all(destination)?;
    }
    std::fs::create_dir_all(destination)?;
    let file = std::fs::File::open(archive)
        .with_context(|| format!("Failed to open '{}'", archive.display()))?;
    if zip {
        zip::ZipArchive::new(file)?
            .extract(destination)
            .context("Failed to extract the NeMo-Speech.cpp archive")?;
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(destination)
            .context("Failed to extract the NeMo-Speech.cpp archive")?;
    }
    Ok(())
}

async fn install(app: tauri::AppHandle, variant: NemotronVariant) -> Result<()> {
    let device = device::detect(&app_data(&app)?.join("models"));
    let runtime = runtime_for_device(&device)
        .ok_or_else(|| anyhow!("NeMo-Speech.cpp has no build for this platform."))?;
    let models = models_dir(&app)?;
    let client = crate::http::http_client();

    let decoder = variant.file();
    let decoder_name = model_file_name(&decoder);
    let encoder_name = model_file_name(&ENCODER);
    let need_runtime = installed_binary(&app, &runtime).is_none();
    let total = decoder.size + ENCODER.size + if need_runtime { runtime.size } else { 0 };
    let done = Arc::new(AtomicU64::new(0));

    let emit = {
        let app = app.clone();
        let done = done.clone();
        move |message: &str, current: u64| {
            let _ = app.emit(
                INSTALL_EVENT,
                InstallProgress {
                    message: message.to_string(),
                    downloaded_bytes: (done.load(Ordering::Relaxed) + current).min(total),
                    total_bytes: total,
                },
            );
        }
    };

    // Free space: fail before downloading gigabytes that cannot fit.
    let missing = (if is_installed(&models, &decoder) {
        0
    } else {
        decoder.size
    }) + (if is_installed(&models, &ENCODER) {
        0
    } else {
        ENCODER.size
    }) + if need_runtime { runtime.size * 3 } else { 0 };
    if let Some(free) = device.free_disk_gb {
        let needed = missing as f64 / GIB + 1.0;
        if free < needed {
            return Err(anyhow!(
                "Not enough disk space: the download needs {needed:.1} GB, {free:.1} GB is free."
            ));
        }
    }

    if need_runtime {
        let archive_path = app_data(&app)?.join("runtimes").join(runtime.file_name());
        let current = AtomicU64::new(0);
        ensure_verified_download(
            &client,
            &VerifiedDownload {
                url: runtime.url(),
                label: "NeMo-Speech.cpp",
                sha256: runtime.sha256,
                size: runtime.size,
            },
            &archive_path,
            &|message| emit(message, current.load(Ordering::Relaxed)),
            &|bytes| {
                current.store(bytes, Ordering::Relaxed);
                emit("Downloading NeMo-Speech.cpp...", bytes);
            },
        )
        .await?;
        done.fetch_add(runtime.size, Ordering::Relaxed);
        emit("Unpacking NeMo-Speech.cpp...", 0);
        let target = runtime_dir(&app, &runtime)?;
        let (archive, zip) = (archive_path.clone(), runtime.zip);
        run_blocking(move || {
            let staging = target.with_extension("partial");
            extract_archive(&archive, &staging, zip)
                .and_then(|()| {
                    if target.exists() {
                        std::fs::remove_dir_all(&target)?;
                    }
                    std::fs::rename(&staging, &target)?;
                    Ok(())
                })
                .map_err(|error| format!("{error:#}"))
        })
        .await
        .map_err(|error| anyhow!(error))?;
        let _ = tokio::fs::remove_file(&archive_path).await;
        if installed_binary(&app, &runtime).is_none() {
            return Err(anyhow!(
                "The NeMo-Speech.cpp archive did not contain '{}'.",
                binary_name()
            ));
        }
    }

    for (file, name) in [(&ENCODER, &encoder_name), (&decoder, &decoder_name)] {
        let download = model_download(file, name);
        let current = AtomicU64::new(0);
        let status_message = format!("Downloading {name}...");
        ensure_verified_download(
            &client,
            &download,
            &models.join(name),
            &|message| emit(message, current.load(Ordering::Relaxed)),
            &|bytes| {
                current.store(bytes, Ordering::Relaxed);
                emit(&status_message, bytes);
            },
        )
        .await?;
        done.fetch_add(file.size, Ordering::Relaxed);
    }
    emit("Nemotron is ready.", 0);
    Ok(())
}

/// Download the runtime and the `variant` model files (resuming any earlier,
/// interrupted download). Progress arrives as `nemotron-install-progress`.
#[tauri::command]
#[specta::specta]
pub async fn install_nemotron(
    app: tauri::AppHandle,
    variant: NemotronVariant,
) -> Result<NemotronStatus, AppError> {
    let task = {
        let mut slot = INSTALL.lock().expect("install lock poisoned");
        if slot.is_some() {
            return Err(AppError::failed("A Nemotron download is already running."));
        }
        let task = tokio::spawn(install(app.clone(), variant));
        *slot = Some(task.abort_handle());
        task
    };
    let outcome = task.await;
    *INSTALL.lock().expect("install lock poisoned") = None;
    match outcome {
        Ok(result) => result?,
        Err(error) if error.is_cancelled() => {
            return Err(AppError::failed(
                "Download cancelled. It resumes where it stopped next time.",
            ))
        }
        Err(error) => return Err(AppError::failed(error.to_string())),
    }
    nemotron_status(app).await
}

/// Stop a running download; what was downloaded so far is kept and resumed.
#[tauri::command]
#[specta::specta]
pub fn cancel_nemotron_install() {
    if let Some(handle) = INSTALL.lock().expect("install lock poisoned").as_ref() {
        handle.abort();
    }
}

/// Delete a downloaded decoder (the shared encoder and runtime stay).
#[tauri::command]
#[specta::specta]
pub async fn remove_nemotron_model(
    app: tauri::AppHandle,
    variant: NemotronVariant,
) -> Result<NemotronStatus, AppError> {
    let models = models_dir(&app)?;
    let name = model_file_name(&variant.file());
    for path in [models.join(&name), models.join(format!("{name}.partial"))] {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AppError::failed(format!(
                    "Failed to delete '{}': {error}",
                    path.display()
                )))
            }
        }
    }
    nemotron_status(app).await
}

// --- Transcription ---------------------------------------------------------

/// `verbatim` keeps fillers and repetitions (to cut them); `readable` drops them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum NemotronMode {
    #[default]
    Verbatim,
    Readable,
}

#[derive(Debug, Default, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", default)]
pub struct NemotronOptions {
    /// `None` uses the recommended build, or any installed one.
    pub variant: Option<NemotronVariant>,
    /// ISO 639-1 code; empty lets the model detect the language.
    pub language: String,
    pub mode: NemotronMode,
    /// The project glossary: names and terms to spell right.
    pub glossary: String,
    /// Label speakers (the model's speaker branch, Sortformer as fallback).
    pub diarize: bool,
    pub sortformer_model_path: String,
}

fn language_name(code: &str) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase().as_str() {
        "en" => "English",
        "de" => "German",
        "fr" => "French",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" => "Portuguese",
        "nl" => "Dutch",
        "pl" => "Polish",
        "cs" => "Czech",
        "sv" => "Swedish",
        "da" => "Danish",
        "fi" => "Finnish",
        "no" | "nb" => "Norwegian",
        "ru" => "Russian",
        "uk" => "Ukrainian",
        "tr" => "Turkish",
        "ar" => "Arabic",
        "hi" => "Hindi",
        "ja" => "Japanese",
        "ko" => "Korean",
        "zh" => "Chinese",
        _ => return None,
    })
}

/// Names and terms from the free-form glossary: one per line or separated by
/// commas/semicolons, with any "term: explanation" cut to the term.
fn glossary_terms(glossary: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut length = 0;
    for raw in glossary.split(['\n', ',', ';']) {
        let term = raw
            .split([':', '='])
            .next()
            .unwrap_or_default()
            .split(" - ")
            .next()
            .unwrap_or_default()
            .trim()
            .trim_start_matches(['-', '*', '•'])
            .trim();
        // `<` and `>` would read as the model's special tags.
        let term: String = term.chars().filter(|c| !matches!(c, '<' | '>')).collect();
        if term.is_empty() || term.len() > 60 || terms.iter().any(|known| known == &term) {
            continue;
        }
        length += term.len() + 2;
        if length > 1_500 {
            break;
        }
        terms.push(term);
    }
    terms
}

/// The instruction sent with the audio, from the model card's prompt guide.
/// It always asks for speaker tags, whether or not speakers are labelled:
/// measured on a two-host podcast, the card's plain "verbatim" prompt
/// transcribed only one host and stopped two thirds in (101 of 233 words),
/// while every speaker-tag prompt kept all of it. The tags themselves never
/// reach the transcript (see [`parse_cli_output`]). A custom instruction
/// replaces the server's prompt, so the language sentence is added here.
fn build_instruction(mode: NemotronMode, language: &str, glossary: &str) -> String {
    let wording = match mode {
        NemotronMode::Verbatim => "Include all spoken words and verbal fillers.",
        NemotronMode::Readable => {
            "Omit fillers and accidental repetitions; keep the remaining spoken wording unchanged."
        }
    };
    let mut instruction = format!(
        "Transcribe this audio with punctuation and capitalization. {wording} Write numbers in their standard written form. Put speaker tags <spk:0> <spk:1> <spk:2> <spk:3> <spk:4> <spk:5> <spk:6> <spk:7> on the transcription."
    );
    if let Some(name) = language_name(language.trim()) {
        instruction.push_str(&format!(" The speech is in {name}."));
    }
    let terms = glossary_terms(glossary);
    if !terms.is_empty() {
        instruction.push_str(&format!(" Context terms: {}.", terms.join(", ")));
    }
    instruction
}

/// KV cache cells for one request of `seconds`: 12.5 tokens per second of
/// audio plus up to 6 per second of transcript, with room for the prompt.
fn context_tokens(seconds: f64) -> u32 {
    let tokens = (seconds.max(1.0) * 19.0).ceil() as u32 + 2_048;
    tokens.max(8_192).div_ceil(256) * 256
}

#[derive(Debug, Deserialize)]
struct CliOutput {
    #[serde(default)]
    text: String,
    #[serde(default)]
    words: Vec<CliWord>,
}

#[derive(Debug, Deserialize)]
struct CliWord {
    word: String,
    start: f32,
    end: f32,
    /// 1-based; absent when the speaker branch did not label the word.
    #[serde(default)]
    speaker: Option<u32>,
}

/// Timed words from the CLI's JSON, shifted by `offset`. Speaker tags the
/// model writes (`<spk:0>`) are not words and are dropped.
fn parse_cli_output(json: &str, offset: f32) -> Result<(Vec<WordWithSpeaker>, bool)> {
    let output: CliOutput =
        serde_json::from_str(json).context("Nemotron wrote JSON the app cannot read")?;
    if output.words.is_empty() && !output.text.trim().is_empty() {
        return Err(anyhow!(
            "Nemotron transcribed the audio but could not time its words, so the transcript cannot be cut. Try again, or use another engine for this file."
        ));
    }
    let tag = regex::Regex::new(r"<[^>]*>").expect("valid regex");
    let mut labelled = false;
    let words = output
        .words
        .into_iter()
        .filter_map(|word| {
            let text = tag.replace_all(&word.word, "").trim().to_string();
            if text.is_empty() {
                return None;
            }
            labelled |= word.speaker.is_some();
            Some(WordWithSpeaker {
                start: word.start + offset,
                end: word.end.max(word.start) + offset,
                text,
                speaker: word
                    .speaker
                    .map_or_else(|| "Speaker 1".to_string(), |n| format!("Speaker {n}")),
            })
        })
        .collect();
    Ok((words, labelled))
}

/// Run one CLI transcription, forwarding its stderr as progress. Cancelling
/// the run kills the process.
async fn run_cli(
    window: &tauri::Window,
    binary: &Path,
    args: &[String],
    run: (u64, &RunControl),
    models: &Path,
) -> Result<()> {
    let mut command = tokio::process::Command::new(binary);
    command
        .args(args)
        .env("NEMO_SPEECH_MODEL_DIR", models)
        .env("NO_COLOR", "1")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to start '{}'", binary.display()))?;
    let (run_id, run_control) = run;
    let pid = child.id();
    if let Some(pid) = pid {
        if let Err(error) = run_control.register_pid(run_id, pid) {
            let _ = child.kill().await;
            return Err(anyhow!(error));
        }
    }

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Failed to capture Nemotron output"))?;
    let mut tail: Vec<String> = Vec::new();
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        log::debug!("{LABEL}: {line}");
        let shown: String = line.chars().take(160).collect();
        let _ = emit_progress(window, &format!("Nemotron: {shown}"));
        tail.push(line);
        if tail.len() > 30 {
            tail.remove(0);
        }
    }
    let status = child.wait().await?;
    if let Some(pid) = pid {
        run_control.clear_pid(run_id, pid);
    }
    run_control
        .ensure_active(run_id)
        .map_err(|error| anyhow!(error))?;
    if !status.success() {
        return Err(anyhow!(
            "Nemotron failed (exit {}): {}",
            status.code().unwrap_or(-1),
            tail.join("\n")
        ));
    }
    Ok(())
}

/// The installed build to run.
fn choose_variant(
    requested: Option<NemotronVariant>,
    recommended: Option<NemotronVariant>,
    installed: &dyn Fn(NemotronVariant) -> bool,
) -> Result<NemotronVariant> {
    if let Some(variant) = requested {
        return if installed(variant) {
            Ok(variant)
        } else {
            Err(anyhow!(
                "The Nemotron {} model is not downloaded. Download it in Settings → Speech engines → Nemotron.",
                variant_short(variant)
            ))
        };
    }
    recommended
        .filter(|&variant| installed(variant))
        .or_else(|| VARIANTS.into_iter().rev().find(|&variant| installed(variant)))
        .ok_or_else(|| {
            anyhow!("Nemotron is not downloaded yet. Download it in Settings → Speech engines → Nemotron (about 21 GB).")
        })
}

/// Transcribe with Nemotron 3.5 Transcribe and return editor-ready segments.
#[tauri::command]
#[specta::specta]
pub async fn transcribe_with_nemotron(
    run_id: u64,
    window: tauri::Window,
    audio_path: String,
    options: NemotronOptions,
    run_control: State<'_, RunControl>,
) -> Result<Vec<TranscriptSegment>, AppError> {
    let run_control = run_control.inner();
    let check = || {
        run_control
            .ensure_active(run_id)
            .map_err(|error| anyhow!(error))
    };
    let app = window.app_handle().clone();
    let run = async {
        check()?;
        let audio_file = PathBuf::from(&audio_path);
        if !audio_file.exists() {
            return Err(anyhow!("Audio file not found: {}", audio_file.display()));
        }
        let device = device::detect(&app_data(&app)?.join("models"));
        let fit = recommend(&device);
        let runtime = runtime_for_device(&device)
            .ok_or_else(|| anyhow!("NeMo-Speech.cpp has no build for this platform."))?;
        let binary = installed_binary(&app, &runtime).ok_or_else(|| {
            anyhow!(
                "Nemotron is not set up yet. Download it in Settings → Speech engines → Nemotron."
            )
        })?;
        let models = models_dir(&app)?;
        if !is_installed(&models, &ENCODER) {
            return Err(anyhow!(
                "The Nemotron encoder is missing. Download Nemotron again in Settings."
            ));
        }
        let variant = choose_variant(options.variant, fit.variant, &|variant| {
            is_installed(&models, &variant.file())
        })?;
        let decoder = models.join(model_file_name(&variant.file()));

        let sortformer_file = if options.diarize {
            // Only needed if the model's own speaker labels are missing, but
            // resolved up front so a failing download fails fast.
            Some(resolve_sortformer_file(&window, &options.sortformer_model_path).await?)
        } else {
            None
        };

        // Long recordings go in chunks the model accepts (one hour at most).
        let duration = crate::silence::probe_duration(&audio_path)
            .await
            .map_err(|error| anyhow!(error))?;
        let chunks = if duration > MAX_CHUNK_SECONDS {
            crate::chunking::split_audio(
                run_id,
                &window,
                audio_path.clone(),
                MAX_CHUNK_SECONDS,
                "",
                run_control,
            )
            .await
            .map_err(|error| anyhow!(error.message))?
        } else {
            vec![crate::chunking::AudioChunk {
                path: audio_path.clone(),
                start_offset: 0.0,
            }]
        };

        let instruction = build_instruction(options.mode, &options.language, &options.glossary);
        let device_flag = if fit.rating == FitRating::Unsupported {
            "auto"
        } else {
            fit.device.flag()
        };
        let tight_gpu = fit.rating == FitRating::Tight && fit.device == RunDevice::Cuda;

        let scratch =
            std::env::temp_dir().join(format!("ai-media-cutter-nemotron-{}", fastrand::u64(..)));
        tokio::fs::create_dir_all(&scratch).await?;
        let outcome = async {
            let mut words: Vec<WordWithSpeaker> = Vec::new();
            let mut labelled = false;
            for (index, chunk) in chunks.iter().enumerate() {
                check()?;
                let part = if chunks.len() > 1 {
                    format!(" (part {}/{})", index + 1, chunks.len())
                } else {
                    String::new()
                };
                emit_progress(&window, &format!("Preparing audio for Nemotron{part}..."))?;
                let wav = scratch.join(format!("chunk{index:03}.wav"));
                let json = scratch.join(format!("chunk{index:03}.json"));
                {
                    let (source, destination) = (PathBuf::from(&chunk.path), wav.clone());
                    run_blocking(move || {
                        write_wav_16k_mono(&source, &destination)
                            .map_err(|error| format!("{error:#}"))
                    })
                    .await
                    .map_err(|error| anyhow!(error))?;
                }
                let chunk_seconds = if chunks.len() > 1 {
                    MAX_CHUNK_SECONDS * 1.2
                } else {
                    duration
                };
                let tokens = context_tokens(chunk_seconds).to_string();
                let mut args: Vec<String> = vec![
                    "transcribe".into(),
                    wav.to_string_lossy().into(),
                    "--model".into(),
                    decoder.to_string_lossy().into(),
                    "--format".into(),
                    "json".into(),
                    "--output".into(),
                    json.to_string_lossy().into(),
                    "--instruction".into(),
                    instruction.clone(),
                    "--device".into(),
                    device_flag.into(),
                    "--no-warmup".into(),
                    "--asr.salm.max_sequences".into(),
                    "1".into(),
                    "--asr.salm.n_ctx_per_sequence".into(),
                    tokens.clone(),
                    "--asr.salm.max_context".into(),
                    tokens,
                ];
                if language_name(options.language.trim()).is_some()
                    || options.language.trim().len() == 2
                {
                    args.extend(["--language".into(), options.language.trim().into()]);
                }
                if options.diarize {
                    args.push("--diarize".into());
                }
                if tight_gpu {
                    for (key, value) in [
                        ("--asr.salm.mtp_draft", "0"),
                        ("--asr.salm.n_batch", "512"),
                        ("--asr.salm.encoder_max_batch", "2"),
                    ] {
                        args.extend([key.into(), value.into()]);
                    }
                }
                emit_progress(
                    &window,
                    &format!(
                        "Transcribing with Nemotron {}{part}...",
                        variant_short(variant)
                    ),
                )?;
                run_cli(&window, &binary, &args, (run_id, run_control), &models).await?;
                let output = tokio::fs::read_to_string(&json)
                    .await
                    .with_context(|| "Nemotron finished without writing its transcript")?;
                let (chunk_words, chunk_labelled) =
                    parse_cli_output(&output, chunk.start_offset as f32)?;
                labelled |= chunk_labelled;
                words.extend(chunk_words);
            }

            // Speaker numbers restart in every chunk, so chunked or unlabelled
            // transcripts are labelled by Sortformer over the whole recording.
            let needs_sortformer = options.diarize && (!labelled || chunks.len() > 1);
            if let (true, Some(file)) = (needs_sortformer && !words.is_empty(), sortformer_file) {
                check()?;
                emit_progress(&window, "Running Sortformer diarization...")?;
                let source = audio_file.clone();
                let diarization = run_blocking(move || {
                    load_audio_16k_mono(&source)
                        .and_then(|audio| diarize(&file, audio))
                        .map_err(|error| format!("{error:#}"))
                })
                .await
                .map_err(|error| anyhow!(error))?;
                for word in &mut words {
                    word.speaker = speaker_label_for_word(word.start, word.end, &diarization);
                }
            }
            Ok::<_, anyhow::Error>(build_transcript_segments(&words))
        }
        .await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        outcome
    };
    Ok(run.await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::GpuInfo;

    fn mac(memory_gb: f64) -> DeviceInfo {
        DeviceInfo {
            os: "macos".into(),
            arch: "aarch64".into(),
            cpu: "Apple M3 Pro".into(),
            cpu_cores: 12,
            total_memory_gb: memory_gb,
            apple_silicon: true,
            gpus: vec![],
            free_disk_gb: Some(100.0),
        }
    }

    fn pc(memory_gb: f64, vram_gb: Option<f64>) -> DeviceInfo {
        DeviceInfo {
            os: "windows".into(),
            arch: "x86_64".into(),
            cpu: "AMD Ryzen".into(),
            cpu_cores: 16,
            total_memory_gb: memory_gb,
            apple_silicon: false,
            gpus: vram_gb
                .map(|memory| GpuInfo {
                    name: "NVIDIA GeForce".into(),
                    vendor: "nvidia".into(),
                    memory_gb: Some(memory),
                })
                .into_iter()
                .collect(),
            free_disk_gb: Some(100.0),
        }
    }

    #[test]
    fn macs_get_a_build_that_fits_the_gpu_share_of_memory() {
        let fit = recommend(&mac(36.0));
        assert_eq!(fit.rating, FitRating::Good);
        assert_eq!(fit.variant, Some(NemotronVariant::Iq4Nl));
        assert_eq!(fit.device, RunDevice::Metal);

        let fit = recommend(&mac(32.0));
        assert_eq!(fit.rating, FitRating::Good);
        assert_eq!(fit.variant, Some(NemotronVariant::Iq4Nl));

        let fit = recommend(&mac(24.0));
        assert_eq!(fit.rating, FitRating::Tight);
        assert_eq!(fit.variant, Some(NemotronVariant::Q2L));

        let fit = recommend(&mac(64.0));
        assert_eq!(fit.variant, Some(NemotronVariant::Iq4Nl));

        let fit = recommend(&mac(16.0));
        assert_eq!(fit.rating, FitRating::Unsupported);
        assert!(fit.message.contains("24 GB"));
    }

    #[test]
    fn nvidia_gpus_are_sized_by_their_own_memory() {
        let fit = recommend(&pc(32.0, Some(24.0)));
        assert_eq!(fit.rating, FitRating::Good);
        assert_eq!(fit.device, RunDevice::Cuda);
        assert_eq!(fit.variant, Some(NemotronVariant::Iq4Nl));

        let fit = recommend(&pc(32.0, Some(16.0)));
        assert_eq!(fit.rating, FitRating::Tight);
        assert_eq!(fit.variant, Some(NemotronVariant::Q2M));

        // A small GPU falls back to the CPU when the RAM allows it.
        let fit = recommend(&pc(64.0, Some(8.0)));
        assert_eq!(fit.rating, FitRating::Slow);
        assert_eq!(fit.device, RunDevice::Cpu);
    }

    #[test]
    fn small_machines_without_a_gpu_are_unsupported() {
        let fit = recommend(&pc(16.0, None));
        assert_eq!(fit.rating, FitRating::Unsupported);
        assert_eq!(fit.variant, None);
    }

    #[test]
    fn runtimes_cover_the_app_platforms() {
        assert_eq!(
            runtime_for("macos", "aarch64", false).unwrap().id,
            "macos-aarch64-metal"
        );
        assert_eq!(
            runtime_for("windows", "x86_64", true).unwrap().id,
            "windows-x86_64-cuda"
        );
        assert_eq!(
            runtime_for("linux", "x86_64", false).unwrap().id,
            "linux-x86_64-cpu"
        );
        assert!(runtime_for("windows", "aarch64", false).is_none());
        assert!(runtime_for("macos", "aarch64", false)
            .unwrap()
            .url()
            .ends_with("/v0.3.0/nemo-speech-0.3.0-macos-aarch64-metal.tar.gz"));
    }

    #[test]
    fn instruction_carries_language_and_glossary_terms() {
        let instruction = build_instruction(
            NemotronMode::Verbatim,
            "de",
            "IAV: engineering company\nNVLink, TensorRT\n- Kowalczyk",
        );
        assert!(instruction.contains("Include all spoken words and verbal fillers."));
        assert!(instruction.contains("<spk:7>"));
        assert!(instruction.contains("The speech is in German."));
        assert!(instruction.ends_with("Context terms: IAV, NVLink, TensorRT, Kowalczyk."));

        let instruction = build_instruction(NemotronMode::Readable, "", "");
        assert!(instruction.contains("Omit fillers"));
        // Speaker tags even without speaker labels: they keep every speaker.
        assert!(instruction.contains("<spk:0>"));
        assert!(!instruction.contains("Context terms"));
        assert!(!instruction.contains("The speech is in"));
    }

    #[test]
    fn glossary_terms_are_deduplicated_and_tag_free() {
        assert_eq!(
            glossary_terms("A <b>, A ; c = d\n\n"),
            vec!["A b".to_string(), "A".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn context_covers_the_request() {
        assert_eq!(context_tokens(10.0), 8_192);
        // One hour: 68,400 tokens plus the prompt allowance.
        assert!(context_tokens(3_600.0) >= 3_600 * 19);
        assert_eq!(context_tokens(3_600.0) % 256, 0);
    }

    #[test]
    fn cli_json_becomes_offset_words_with_speakers() {
        let json = r#"{
  "file": "a.wav",
  "text": "<spk:0> Hello there. <spk:1> Hi.",
  "confidence": 0,
  "duration": 3.2,
  "languages": ["en"],
  "words": [
    {"word": "<spk:0>", "start": 0.1, "end": 0.1, "confidence": 0},
    {"word": "Hello", "start": 0.2, "end": 0.5, "confidence": 0, "speaker": 1},
    {"word": "there.", "start": 0.5, "end": 0.9, "confidence": 0, "speaker": 1},
    {"word": "Hi.", "start": 1.4, "end": 1.6, "confidence": 0, "speaker": 2}
  ]
}"#;
        let (words, labelled) = parse_cli_output(json, 10.0).unwrap();
        assert!(labelled);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].text, "Hello");
        assert!((words[0].start - 10.2).abs() < 1e-4);
        assert_eq!(words[2].speaker, "Speaker 2");

        let segments = build_transcript_segments(&words);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "Hello there.");
    }

    /// Output of `nemo-speech 0.3.0 transcribe --format json --diarize` with the
    /// IQ4_NL build on `test_podcast.m4a` (74 s, two hosts), captured on an M3 Pro.
    #[test]
    fn real_cli_output_becomes_a_two_speaker_transcript() {
        let json = include_str!("../../dev-resources/test-data/nemotron_cli_output.json");
        let (words, labelled) = parse_cli_output(json, 0.0).unwrap();
        assert!(labelled);
        assert_eq!(words.len(), 233);
        assert!(words.windows(2).all(|pair| pair[0].start <= pair[1].start));
        assert!(words.iter().all(|word| !word.text.contains('<')));

        let segments = build_transcript_segments(&words);
        let speakers: std::collections::BTreeSet<_> = segments
            .iter()
            .map(|segment| segment.speaker.as_str())
            .collect();
        assert_eq!(
            speakers.into_iter().collect::<Vec<_>>(),
            ["Speaker 1", "Speaker 2"]
        );
        assert!(segments[0].text.starts_with("Let's unpack this."));
    }

    #[test]
    fn unlabelled_words_default_to_one_speaker() {
        let json = r#"{"text": "Hi", "words": [{"word": "Hi", "start": 0.0, "end": 0.2, "confidence": 0}]}"#;
        let (words, labelled) = parse_cli_output(json, 0.0).unwrap();
        assert!(!labelled);
        assert_eq!(words[0].speaker, "Speaker 1");
    }

    #[test]
    fn a_transcript_without_timings_is_an_error() {
        let json = r#"{"text": "Hello there", "words": []}"#;
        let error = parse_cli_output(json, 0.0).unwrap_err().to_string();
        assert!(error.contains("could not time"));
        // Silence is not an error: an empty transcript.
        let (words, _) = parse_cli_output(r#"{"text": "", "words": []}"#, 0.0).unwrap();
        assert!(words.is_empty());
    }

    #[test]
    fn the_run_uses_the_requested_build_or_an_installed_one() {
        let only_q2 = |variant: NemotronVariant| variant == NemotronVariant::Q2L;
        assert_eq!(
            choose_variant(None, Some(NemotronVariant::Iq4Nl), &only_q2).unwrap(),
            NemotronVariant::Q2L
        );
        assert!(choose_variant(Some(NemotronVariant::Q8), None, &only_q2)
            .unwrap_err()
            .to_string()
            .contains("Q8_0"));
        assert!(choose_variant(None, None, &|_| false)
            .unwrap_err()
            .to_string()
            .contains("not downloaded"));
    }

    #[test]
    fn pinned_files_have_the_published_names() {
        assert_eq!(
            model_file_name(&ENCODER),
            "NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-Encoder-F16.gguf"
        );
        assert_eq!(
            model_file_name(&NemotronVariant::Iq4Nl.file()),
            "NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-AD-IQ4_NL.gguf"
        );
        for variant in VARIANTS {
            assert_eq!(variant.file().sha256.len(), 64);
        }
    }
}
