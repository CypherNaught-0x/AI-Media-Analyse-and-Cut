use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

pub const RUN_CANCELLED_MESSAGE: &str = "Run cancelled.";

#[derive(Default)]
pub struct RunControl {
    current_run_id: AtomicU64,
    cancelled_run_id: AtomicU64,
    active_pid: Mutex<Option<(u64, u32)>>,
}

impl RunControl {
    pub fn begin_run(&self) -> u64 {
        let run_id = self.current_run_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.cancelled_run_id.store(0, Ordering::SeqCst);
        *self.active_pid.lock().expect("active pid lock poisoned") = None;
        run_id
    }

    pub fn cancel_current_run(&self) -> Result<(), String> {
        let run_id = self.current_run_id.load(Ordering::SeqCst);
        if run_id == 0 {
            return Ok(());
        }

        self.cancelled_run_id.store(run_id, Ordering::SeqCst);

        if let Some((active_run_id, pid)) = *self.active_pid.lock().expect("active pid lock poisoned") {
            if active_run_id == run_id {
                kill_process(pid)?;
            }
        }

        Ok(())
    }

    pub fn is_cancelled(&self, run_id: u64) -> bool {
        run_id == 0
            || self.current_run_id.load(Ordering::SeqCst) != run_id
            || self.cancelled_run_id.load(Ordering::SeqCst) == run_id
    }

    pub fn ensure_active(&self, run_id: u64) -> Result<(), String> {
        if self.is_cancelled(run_id) {
            Err(RUN_CANCELLED_MESSAGE.to_string())
        } else {
            Ok(())
        }
    }

    pub fn register_pid(&self, run_id: u64, pid: u32) -> Result<(), String> {
        {
            let mut active_pid = self.active_pid.lock().expect("active pid lock poisoned");
            *active_pid = Some((run_id, pid));
        }

        if self.is_cancelled(run_id) {
            let _ = kill_process(pid);
            return Err(RUN_CANCELLED_MESSAGE.to_string());
        }

        Ok(())
    }

    pub fn clear_pid(&self, run_id: u64, pid: u32) {
        let mut active_pid = self.active_pid.lock().expect("active pid lock poisoned");
        if matches!(*active_pid, Some((active_run_id, active_pid_value)) if active_run_id == run_id && active_pid_value == pid) {
            *active_pid = None;
        }
    }

    pub async fn run_cancellable<T, E, F>(&self, run_id: u64, future: F) -> Result<T, String>
    where
        E: ToString,
        F: Future<Output = Result<T, E>>,
    {
        tokio::pin!(future);

        loop {
            self.ensure_active(run_id)?;

            tokio::select! {
                result = &mut future => return result.map_err(|error| error.to_string()),
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
    }
}

fn kill_process(pid: u32) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .map_err(|error| format!("Failed to cancel run: {}", error))?;

        if status.success() {
            return Ok(());
        }

        return Err(format!("Failed to cancel run (taskkill exited with {})", status));
    }

    #[cfg(not(target_os = "windows"))]
    {
        let status = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map_err(|error| format!("Failed to cancel run: {}", error))?;

        if status.success() {
            return Ok(());
        }

        Err(format!("Failed to cancel run (kill exited with {})", status))
    }
}
