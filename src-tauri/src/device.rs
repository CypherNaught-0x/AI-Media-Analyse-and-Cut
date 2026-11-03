//! What this machine can run: memory, GPUs and free disk space, for the setup
//! wizard and the Nemotron model choice.

use serde::Serialize;
use std::path::Path;
use sysinfo::{Disks, MemoryRefreshKind, RefreshKind, System};
use tauri::Manager;

use crate::error::AppError;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    /// `nvidia`, `apple`, ...
    pub vendor: String,
    /// Dedicated memory; `None` for unified memory (Apple Silicon).
    #[specta(type = Option<specta_typescript::Number>)]
    pub memory_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// `macos`, `windows`, `linux`.
    pub os: String,
    /// `aarch64`, `x86_64`.
    pub arch: String,
    pub cpu: String,
    pub cpu_cores: u32,
    #[specta(type = specta_typescript::Number)]
    pub total_memory_gb: f64,
    /// Apple Silicon shares one memory pool between CPU and GPU.
    pub apple_silicon: bool,
    pub gpus: Vec<GpuInfo>,
    /// Free space on the disk that holds the app's models.
    #[specta(type = Option<specta_typescript::Number>)]
    pub free_disk_gb: Option<f64>,
}

impl DeviceInfo {
    /// The largest NVIDIA GPU's memory, if there is one.
    pub fn nvidia_memory_gb(&self) -> Option<f64> {
        self.gpus
            .iter()
            .filter(|gpu| gpu.vendor == "nvidia")
            .filter_map(|gpu| gpu.memory_gb)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }
}

/// `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`
/// output (memory in MiB), one GPU per line.
fn parse_nvidia_smi(output: &str) -> Vec<GpuInfo> {
    output
        .lines()
        .filter_map(|line| {
            let (name, memory) = line.rsplit_once(',')?;
            let mib: f64 = memory.trim().parse().ok()?;
            Some(GpuInfo {
                name: name.trim().to_string(),
                vendor: "nvidia".to_string(),
                memory_gb: Some(mib / 1024.0),
            })
        })
        .collect()
}

fn nvidia_gpus() -> Vec<GpuInfo> {
    let mut command = std::process::Command::new("nvidia-smi");
    command.args([
        "--query-gpu=name,memory.total",
        "--format=csv,noheader,nounits",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    match command.output() {
        Ok(output) if output.status.success() => {
            parse_nvidia_smi(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// Free space on the disk whose mount point is the longest prefix of `path`.
fn free_disk_gb(path: &Path) -> Option<f64> {
    let disks = Disks::new_with_refreshed_list().ok()?;
    disks
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space() as f64 / GIB)
}

pub(crate) fn detect(models_dir: &Path) -> DeviceInfo {
    let total_memory = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    )
    .map_or(0, |system| system.total_memory());
    let cpu = System::new()
        .ok()
        .and_then(|mut system| {
            system.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
            system
                .cpus()
                .first()
                .map(|cpu| cpu.brand().trim().to_string())
        })
        .unwrap_or_default();
    let apple_silicon = cfg!(all(target_os = "macos", target_arch = "aarch64"));

    let mut gpus = Vec::new();
    if apple_silicon {
        gpus.push(GpuInfo {
            name: if cpu.is_empty() {
                "Apple Silicon GPU".to_string()
            } else {
                format!("{cpu} GPU")
            },
            vendor: "apple".to_string(),
            memory_gb: None,
        });
    } else {
        gpus.extend(nvidia_gpus());
    }

    // The models folder may not exist yet; measure the nearest existing parent.
    let existing = models_dir
        .ancestors()
        .find(|path| path.exists())
        .unwrap_or(models_dir);

    DeviceInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu,
        cpu_cores: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
        total_memory_gb: total_memory as f64 / GIB,
        apple_silicon,
        gpus,
        free_disk_gb: free_disk_gb(existing),
    }
}

pub(crate) fn models_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, AppError> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("models"))
        .map_err(|error| AppError::failed(error.to_string()))
}

/// Memory, GPUs and free disk space of this machine.
#[tauri::command]
#[specta::specta]
pub async fn detect_device(app: tauri::AppHandle) -> Result<DeviceInfo, AppError> {
    let models = models_dir(&app)?;
    tokio::task::spawn_blocking(move || detect(&models))
        .await
        .map_err(|error| AppError::failed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_lines_become_gpus() {
        let gpus =
            parse_nvidia_smi("NVIDIA GeForce RTX 4090, 24564\nNVIDIA A100-SXM4-80GB, 81920\n");
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert!((gpus[0].memory_gb.unwrap() - 23.99).abs() < 0.01);
        assert_eq!(gpus[1].memory_gb, Some(80.0));
    }

    #[test]
    fn unparsable_nvidia_smi_output_is_ignored() {
        assert!(parse_nvidia_smi("NVIDIA-SMI has failed\n").is_empty());
        assert!(parse_nvidia_smi("").is_empty());
    }
}
