import { invoke } from '@tauri-apps/api/core';
import { native } from './bridge';

export type IndexServiceAction = 'install' | 'start' | 'uninstall';
export type IndexServiceIssue =
  | 'unsupported'
  | 'isolated'
  | 'unsafeLocation'
  | 'existingBinary'
  | 'unrecognizedService'
  | 'unavailable'
  | 'busy'
  | 'cancelled'
  | 'permissionDenied'
  | 'timedOut'
  | 'disabled'
  | 'failed';
export interface IndexServiceStatus {
  state: 'unsupported' | 'notInstalled' | 'stopped' | 'running' | 'unavailable';
  issue: IndexServiceIssue | null;
  managementAllowed: boolean;
  busy: boolean;
  canInstall: boolean;
  canStart: boolean;
  canRemove: boolean;
}
export function indexServiceIssue(error: unknown): IndexServiceIssue {
  if (typeof error === 'object' && error !== null && 'code' in error) {
    const code = (error as { code: unknown }).code;
    if (
      typeof code === 'string' &&
      [
        'unsupported',
        'isolated',
        'unsafeLocation',
        'existingBinary',
        'unrecognizedService',
        'unavailable',
        'busy',
        'cancelled',
        'permissionDenied',
        'timedOut',
        'disabled',
        'failed',
      ].includes(code)
    )
      return code as IndexServiceIssue;
  }
  return 'unavailable';
}
export const indexService = {
  async status(): Promise<IndexServiceStatus> {
    if (!native)
      return {
        state: 'unsupported',
        issue: 'unsupported',
        managementAllowed: false,
        busy: false,
        canInstall: false,
        canStart: false,
        canRemove: false,
      };
    return invoke('index_service_status');
  },
  async manage(action: IndexServiceAction): Promise<IndexServiceStatus> {
    if (!native) throw { code: 'unsupported' };
    return invoke('index_service_manage', { action });
  },
};
