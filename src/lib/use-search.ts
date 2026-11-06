import { useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { bridge, native } from './bridge';
import { errorMessage, setSession, showToast, submitSearch, useClient } from './store';
import { translate } from './i18n';
import { isComposing } from './types';
import { fileShortcutLabel, matchesFileShortcut } from './file-shortcut';
import type { IndexStatus, SearchResponse } from './types';
import { parseInput } from './utilities';
import type { UtilityAnswer } from './utilities';
import { pasteClipboard, useClipboard } from './clipboard';
import { useDictionary } from './dictionary';
import { useContentSearch } from './content';
import { automaticBodyEligible } from './automatic-search';
import { useIndexActivity } from './index-activity';

const emptyStatus: IndexStatus = {
  scanning: false,
  initialScanComplete: false,
  count: 0,
  revision: 0,
  skipped: 0,
  warnings: [],
  watching: false,
};
export function useSearch(quick = false) {
  const listId = useId();
  const { settings, session, runtime } = useClient();
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const request = parseInput(session.query);
  const clips = useClipboard(request.kind === 'clipboard', request.input);
  const dict = useDictionary(
    request.kind === 'dictionary',
    request.input,
    request.dictionaryLanguage,
    settings.language,
  );
  const activity = useIndexActivity();
  const [localStatus, setStatus] = useState(emptyStatus);
  const status = activity?.files ?? localStatus;
  const [response, setResponse] = useState<{ key: string; data: SearchResponse } | null>(null);
  const [failure, setFailure] = useState<{ key: string; message: string } | null>(null);
  const [loading, setLoading] = useState(false);
  const [answerState, setAnswerState] = useState<{
    key: string;
    answer?: UtilityAnswer;
    error?: 'invalid' | 'timeout';
  } | null>(null);
  const scopes = settings.scopes.filter(
    (scope) => scope.filenameEnabled && (!session.scopeId || scope.id === session.scopeId),
  );
  const scopesKey = scopes.map((scope) => `${scope.id}:${scope.path}`).join('|');
  const key = JSON.stringify([
    session.query,
    session.extension,
    session.scopeId,
    scopesKey,
    settings.keyboardCorrection,
    session.mode,
  ]);
  const utilityKey = JSON.stringify([session.query, settings.calculator]);
  const utilityWorker = useRef<{ worker: Worker; ready: boolean } | null>(null);
  useEffect(
    () => () => {
      utilityWorker.current?.worker.terminate();
      utilityWorker.current = null;
    },
    [],
  );
  useEffect(() => {
    if (!native || activity) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = await bridge.indexStatus();
        if (cancelled) return;
        setStatus((previous) =>
          previous.revision === next.revision &&
          previous.scanning === next.scanning &&
          previous.initialScanComplete === next.initialScanComplete
            ? previous
            : next,
        );
        timer = setTimeout(() => void poll(), next.scanning ? 400 : 1000);
      } catch (cause) {
        if (!cancelled) {
          showToast(errorMessage(cause));
          timer = setTimeout(() => void poll(), 3000);
        }
      }
    };
    void poll();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [scopesKey, activity !== null]);
  useEffect(() => {
    if (
      request.kind !== 'files' ||
      (session.mode !== 'filename' && session.mode !== 'auto') ||
      (quick && !request.input) ||
      !scopesKey ||
      !native
    ) {
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    const timer = setTimeout(() => {
      void bridge
        .searchFiles(request.input, session.extension, session.scopeId)
        .then((data) => {
          if (!cancelled) {
            setResponse({ key, data });
            setFailure(null);
          }
        })
        .catch((cause) => {
          if (!cancelled) setFailure({ key, message: errorMessage(cause) });
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, 100);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [
    key,
    request.kind,
    request.input,
    scopesKey,
    session.extension,
    session.scopeId,
    status.revision,
    quick,
    session.mode,
  ]);
  useEffect(() => {
    if (!['calculator', 'units'].includes(request.kind) || !request.input) return;
    let worker: Worker | undefined;
    let timeout: ReturnType<typeof setTimeout>;
    let cancelled = false;
    let completed = false;
    const timer = setTimeout(() => {
      const cached = utilityWorker.current;
      worker =
        cached?.worker ??
        new Worker(new URL('./utility.worker.ts', import.meta.url), { type: 'module' });
      const workerState = cached ?? { worker, ready: false };
      utilityWorker.current = workerState;
      const expired = () => {
        worker?.terminate();
        utilityWorker.current = null;
        completed = true;
        if (!cancelled) setAnswerState({ key: utilityKey, error: 'timeout' });
      };
      // Cold module loading has its own budget; the execution deadline starts after the worker is ready.
      timeout = setTimeout(expired, workerState.ready ? 2000 : 10000);
      worker.onmessage = (
        event: MessageEvent<{ ready?: boolean; answer?: UtilityAnswer; error?: boolean }>,
      ) => {
        clearTimeout(timeout);
        if (event.data.ready) {
          workerState.ready = true;
          timeout = setTimeout(expired, 2000);
          return;
        }
        completed = true;
        if (!cancelled)
          setAnswerState({
            key: utilityKey,
            ...(event.data.error ? { error: 'invalid' as const } : { answer: event.data.answer }),
          });
      };
      worker.onerror = () => {
        clearTimeout(timeout);
        worker?.terminate();
        utilityWorker.current = null;
        completed = true;
        if (!cancelled) setAnswerState({ key: utilityKey, error: 'invalid' });
      };
      worker.postMessage({ input: request.input, kind: request.kind, ...settings.calculator });
    }, 160);
    return () => {
      cancelled = true;
      clearTimeout(timer);
      clearTimeout(timeout);
      if (!completed && worker) {
        worker.terminate();
        utilityWorker.current = null;
      }
    };
  }, [utilityKey, request.kind, request.input, settings.calculator]);
  const data = response?.key === key ? response.data : null;
  const error = failure?.key === key ? failure.message : null;
  const utility = answerState?.key === utilityKey ? answerState : null;
  const automaticFallback =
    native &&
    session.mode === 'auto' &&
    request.kind === 'files' &&
    automaticBodyEligible(request.input) &&
    settings.scopes.some(
      (scope) => scope.contentEnabled && (!session.scopeId || scope.id === session.scopeId),
    ) &&
    (!scopesKey || (data !== null && data.total === 0 && !error));
  const bodyActive =
    request.kind === 'files' &&
    (session.mode === 'content' || session.mode === 'semantic' || automaticFallback);
  const body = useContentSearch(bodyActive);
  const awaitingFilename =
    native &&
    request.kind === 'files' &&
    (session.mode === 'auto' || session.mode === 'filename') &&
    Boolean(scopesKey) &&
    (!quick || Boolean(request.input)) &&
    !data &&
    !error;
  const selected =
    data?.results.find((file) => file.path === session.selectedPath) ?? data?.results[0];
  const report = (cause: unknown) =>
    showToast(
      errorMessage(cause) === 'NATIVE_ONLY'
        ? t('nativeOnly')
        : errorMessage(cause) === 'OFFLINE'
          ? t('offline')
          : errorMessage(cause),
    );
  const copy = async (text: string) => {
    await bridge.copyText(text);
    showToast(t('copied'));
  };
  const activate = async (path?: string) => {
    if (!session.query.trim() && !selected && !path) return;
    await submitSearch();
    if (request.kind === 'web' && request.input)
      await bridge.webSearch(request.provider!, request.input);
    else if (request.kind === 'clipboard' && clips.selected) {
      await pasteClipboard(clips.selected.id, settings.language);
      return;
    } else if (request.kind === 'dictionary') {
      await dict.lookup();
      return;
    } else if (utility?.answer) await copy(utility.answer.value);
    else if (request.kind === 'files' && bodyActive && body.selected && !body.error)
      await body.open(path ?? body.selected.chunk.filePath);
    else if (request.kind === 'files' && !bodyActive && (path || selected) && !error)
      await bridge.fileAction(path ?? selected!.path);
    else return;
    if (quick) await bridge.hideQuick();
  };
  const keyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (isComposing(event.nativeEvent)) return;
    if (
      request.kind === 'files' &&
      matchesFileShortcut(settings.fileOpenShortcut, event.nativeEvent, runtime.platform)
    ) {
      event.preventDefault();
      event.stopPropagation();
      void activate().catch(report);
      return;
    }
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (request.kind === 'files' && bodyActive) {
        event.preventDefault();
        body.move(event.key === 'ArrowDown' ? 1 : -1);
        return;
      }
      if (request.kind === 'clipboard') {
        event.preventDefault();
        clips.move(event.key === 'ArrowDown' ? 1 : -1);
        return;
      }
      const files = data?.results ?? [];
      if (!files.length) return;
      event.preventDefault();
      const current = files.findIndex((file) => file.path === selected?.path);
      const index = (current + (event.key === 'ArrowDown' ? 1 : -1) + files.length) % files.length;
      setSession({ selectedPath: files[index].path });
    } else if (event.key === 'Enter' && request.kind !== 'files' && !event.repeat) {
      event.preventDefault();
      void activate().catch(report);
    }
  };
  return {
    listId,
    selectedId:
      request.kind === 'files' && bodyActive
        ? body.selected
          ? `${body.listId}-${body.selected.chunk.id}`
          : undefined
        : request.kind === 'clipboard'
          ? clips.selected
            ? `${clips.listId}-${clips.selected.id}`
            : undefined
          : selected && data
            ? `${listId}-${data.results.indexOf(selected)}`
            : undefined,
    clips,
    dict,
    body,
    bodyActive,
    request,
    status: data?.status.revision && data.status.revision > status.revision ? data.status : status,
    data,
    error,
    loading: loading || awaitingFilename,
    utility,
    selected,
    scopes,
    keyDown,
    activate: () => void activate().catch(report),
    openFile: (path: string) => void activate(path).catch(report),
    copy: (text: string) => void copy(text).catch(report),
    reveal: () => {
      if (selected) void bridge.fileAction(selected.path, true).catch(report);
    },
    refresh: () => void bridge.refreshIndex().catch(report),
    t,
    language: settings.language,
    fileOpenShortcut: fileShortcutLabel(settings.fileOpenShortcut, runtime.platform),
    session,
  };
}
export type SearchController = ReturnType<typeof useSearch>;
