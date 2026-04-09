import { invoke } from '@tauri-apps/api/core';

export const RUN_CANCELLED_MESSAGE = 'Run cancelled.';

export function isRunCancelled(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes(RUN_CANCELLED_MESSAGE);
}

export async function beginRun(): Promise<number> {
  return invoke<number>('begin_run');
}
