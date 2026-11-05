export interface OcrRuntimeStatus {
  phase:
    | 'unsupported'
    | 'notInstalled'
    | 'installing'
    | 'ready'
    | 'starting'
    | 'running'
    | 'cancelled'
    | 'error';
  pipeline: string;
  target: string;
  available: boolean;
  downloadedBytes: number;
  totalBytes: number;
  expandedBytes: number;
  processedFiles: number;
  totalFiles: number;
  pid: number | null;
  lastError: string | null;
}
