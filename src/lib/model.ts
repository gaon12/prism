import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { native } from './bridge';

export type EmbeddingPhase =
  'notInstalled' | 'downloading' | 'ready' | 'starting' | 'running' | 'cancelled' | 'error';

/** Exact camelCase DTO from embedding_runtime::EmbeddingStatus. */
export interface EmbeddingStatus {
  phase: EmbeddingPhase;
  modelId: string;
  modelRevision: string;
  runtimeVersion: string;
  runtimeTarget: string | null;
  runtimeFilename: string | null;
  runtimeBytes: number | null;
  quantization: string | null;
  modelReady: boolean;
  runtimeReady: boolean;
  downloadedBytes: number;
  totalBytes: number;
  pid: number | null;
  dimensions: number;
  lastError: string | null;
}

export const embeddingAssets = {
  modelId: 'google/embeddinggemma-2',
  modelUrl: 'https://huggingface.co/google/embeddinggemma-2',
  ggufUrl:
    'https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/tree/6ea3e3a309d12a4779cfda98b8c19800d2d5da71',
  runtimeUrl: 'https://github.com/ggml-org/llama.cpp/releases/tag/b11960',
  runtimeVersion: 'b11960',
  modelFiles: ['embeddinggemma-2-Q8_0.gguf', 'embeddinggemma-2-BF16.gguf'],
} as const;

function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!native) return Promise.reject(new Error('NATIVE_ONLY'));
  return invoke<T>(command, args);
}

async function importAsset(runtime: boolean, title?: string): Promise<EmbeddingStatus | null> {
  if (!native) throw new Error('NATIVE_ONLY');
  const path = await open({
    title,
    multiple: false,
    directory: false,
    filters: [
      {
        name: runtime ? 'CPU runtime (ZIP / TAR.GZ)' : 'GGUF',
        extensions: runtime ? ['zip', 'gz'] : ['gguf'],
      },
    ],
  });
  return path ? call<EmbeddingStatus>('model_import', { path, runtime }) : null;
}

/** Construction/importing this module never downloads files or starts a server. */
export const model = {
  status: () => call<EmbeddingStatus>('model_status'),
  cancel: () => call<void>('model_cancel'),
  download: () => call<EmbeddingStatus>('model_download'),
  start: () => call<EmbeddingStatus>('model_start'),
  stop: () => call<EmbeddingStatus>('model_stop'),
  importModel: (title?: string) => importAsset(false, title),
  importRuntime: (title?: string) => importAsset(true, title),
  openSource: (url: string) => call<void>('dictionary_open_source', { url }),
};
