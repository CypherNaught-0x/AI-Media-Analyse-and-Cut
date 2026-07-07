// The app entry is loaded dynamically so that screenshot demo mode (which
// seeds localStorage and mocks the Tauri IPC layer) can run before any module
// reads persisted state at import time.
async function bootstrap() {
  if (import.meta.env.VITE_SCREENSHOT_DEMO) {
    const { setupDemoMode } = await import("./demo/screenshotDemo");
    setupDemoMode();
  }
  const { mountApp } = await import("./app");
  mountApp();
}

bootstrap();
