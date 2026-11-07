import { invoke } from '@tauri-apps/api/core';

export interface OfficeStatus {
  supported: boolean;
  /** Files are present. Integrity is checked again on every rendering request. */
  installed: boolean;
  installing: boolean;
  archiveBytes: number;
  expandedBytes: number;
  downloadAvailable: boolean;
  progress: { phase: string; files: number; totalFiles: number } | null;
  lastError: string | null;
  failureDetails?: { stage: string | null; nativeCode: number | null } | null;
  revision: number;
}
export const officeEngine = {
  status: () => invoke<OfficeStatus>('collection_office_status'),
  install: (path: string) => invoke<void>('collection_office_install', { path }),
  cancel: () => invoke<void>('collection_office_cancel'),
};
