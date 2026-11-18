// Behaviour of the setup window. Plain JavaScript (no build step) so the setup stays small.
//
// Flow:  loading → options (or uninstall) → progress → done
// - In "passive" mode (started by Prism's updater) the options screen is skipped.
// - Explicit local/file ?preview=... URLs use fixtures. A missing native bridge
//   in an installed app fails closed instead of simulating installation.

(function () {
  'use strict';

  const tauri = window.__TAURI__;
  const $ = (id) => document.getElementById(id);
  const root = document.querySelector('.setup');

  // ---------- Language ----------
  const languages = ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'];
  const previewModes = new Set([
    'options',
    'progress',
    'done',
    'uninstall',
    'restore',
    'error',
    'installed',
    'damaged',
    'recovery',
    'conflict',
  ]);
  const query = new URLSearchParams(location.search);
  const localPreviewOrigin =
    location.protocol === 'file:' ||
    (['http:', 'https:'].includes(location.protocol) &&
      ['localhost', '127.0.0.1', '[::1]'].includes(location.hostname));
  const preview =
    !tauri && localPreviewOrigin && previewModes.has(query.get('preview')) ? query : null;
  function readPreference(key) {
    try {
      return localStorage.getItem('prism.setup.' + key);
    } catch {
      return null;
    }
  }
  function savePreference(key, value) {
    try {
      localStorage.setItem('prism.setup.' + key, value);
    } catch {
      /* The choice still works for this run. */
    }
  }
  function pickLanguage() {
    const wanted = (navigator.language || 'en').toLowerCase();
    if (wanted.startsWith('zh')) {
      // Traditional Chinese for Taiwan, Hong Kong and Macau; Simplified otherwise.
      return /tw|hk|mo|hant/.test(wanted) ? 'zh-TW' : 'zh-CN';
    }
    const base = wanted.slice(0, 2);
    return languages.includes(base) ? base : 'en';
  }
  const preferredLanguage = preview?.get('lang') || readPreference('language');
  let language = languages.includes(preferredLanguage) ? preferredLanguage : pickLanguage();
  document.documentElement.lang = language;
  let index = languages.indexOf(language);
  const systemTheme = matchMedia('(prefers-color-scheme: dark)');
  let theme = preview?.get('theme') || readPreference('theme') || 'system';
  if (!['system', 'light', 'dark'].includes(theme)) theme = 'system';
  function applyTheme() {
    document.documentElement.dataset.theme =
      theme === 'system' ? (systemTheme.matches ? 'dark' : 'light') : theme;
    $('theme').value = theme;
  }
  applyTheme();
  systemTheme.addEventListener('change', applyTheme);

  /** Text for `key` in the chosen language, with `{name}` placeholders filled from `values`. */
  function t(key, values) {
    const list = window.SETUP_MESSAGES[key];
    let text = (list && (list[index] || list[1])) || key;
    for (const [name, value] of Object.entries(values || {})) {
      text = text.replaceAll('{' + name + '}', value);
    }
    return text;
  }

  function renderLabels() {
    document.documentElement.lang = language;
    $('language').value = language;
    for (const element of document.querySelectorAll('[data-text]'))
      element.textContent = t(element.dataset.text);
    for (const element of document.querySelectorAll('[data-label]')) {
      element.title = t(element.dataset.label);
      element.setAttribute('aria-label', element.title);
    }
  }
  renderLabels();

  // ---------- Backend ----------
  let backendUnavailable = false;
  let backend;
  try {
    if (
      typeof tauri?.core?.invoke === 'function' &&
      typeof tauri?.event?.listen === 'function' &&
      typeof tauri?.window?.getCurrentWindow === 'function'
    )
      backend = realBackend();
    else if (preview) backend = previewBackend();
    else {
      backendUnavailable = true;
      backend = unavailableBackend();
    }
  } catch {
    backendUnavailable = true;
    backend = unavailableBackend();
  }

  function unavailableBackend() {
    const unavailable = async () => {
      throw new Error(t('bridgeUnavailable'));
    };
    return {
      info: unavailable,
      checkFolder: unavailable,
      chooseFolder: unavailable,
      install: unavailable,
      uninstall: unavailable,
      restore: unavailable,
      repair: unavailable,
      scan: unavailable,
      readLog: unavailable,
      openLogFolder: unavailable,
      finish: unavailable,
      onProgress: unavailable,
      minimize: unavailable,
      close: unavailable,
    };
  }

  function realBackend() {
    const invoke = tauri.core.invoke;
    const appWindow = tauri.window.getCurrentWindow();
    return {
      info: () => invoke('setup_info'),
      checkFolder: (path) => invoke('check_folder', { path }),
      chooseFolder: (current) => invoke('choose_folder', { current }),
      install: (options, closeRunningApp = false) =>
        invoke('run_install', { options, closeRunningApp }),
      uninstall: (removeData, closeRunningApp = false) =>
        invoke('run_uninstall', { removeData, closeRunningApp }),
      restore: (installDir, expectedVersion, closeRunningApp = false) =>
        invoke('run_restore', { installDir, expectedVersion, closeRunningApp }),
      repair: (installDir, closeRunningApp = false) =>
        invoke('run_repair', { installDir, closeRunningApp }),
      scan: (installDir) => invoke('scan_installation', { installDir }),
      readLog: () => invoke('read_log'),
      openLogFolder: () => invoke('open_log_folder'),
      finish: (launch) => invoke('finish', { launch }),
      onProgress: (handler) =>
        tauri.event.listen('setup://progress', (event) => handler(event.payload)),
      minimize: () => appWindow.minimize(),
      close: () => appWindow.close(),
    };
  }

  // Fake backend for designing in a browser. Never used inside the real setup.
  function previewBackend() {
    const screen = preview.get('preview') || 'options';
    let listener = () => {};
    const fakeRun = () =>
      new Promise((resolve) => {
        let fraction = 0;
        const timer = setInterval(() => {
          fraction = Math.min(1, fraction + 0.04);
          listener({ stage: fraction < 0.85 ? 'copying' : 'registering', fraction });
          if (fraction >= 1) {
            clearInterval(timer);
            resolve();
          }
        }, 80);
      });
    return {
      info: async () => ({
        mode: screen === 'uninstall' ? 'uninstall' : screen === 'restore' ? 'restore' : 'install',
        ui: screen === 'progress' ? 'passive' : 'full',
        version: '0.2.0',
        installedVersion: ['installed', 'damaged', 'recovery', 'restore'].includes(screen)
          ? '0.2.0'
          : null,
        repairAvailable: ['installed', 'damaged'].includes(screen),
        recoveryPending: screen === 'recovery',
        previousVersion:
          screen === 'restore'
            ? { currentVersion: '0.2.0', version: '0.1.0', requiredBytes: 14200000 }
            : null,
        installDir: 'C:\\Users\\you\\AppData\\Local\\Programs\\Prism',
        defaultDir: 'C:\\Users\\you\\AppData\\Local\\Programs\\Prism',
        desktopShortcut: true,
        startMenuShortcut: true,
        requiredBytes: 14_200_000,
        freeBytes: 212_000_000_000,
        payloadReady: true,
        relaunch: false,
      }),
      checkFolder: async () => ({
        error:
          screen === 'conflict' ? 'Preview: the selected folder contains unrelated files.' : null,
        freeBytes: 212_000_000_000,
        recoveryPending: screen === 'recovery',
        health:
          screen === 'installed'
            ? 'installed'
            : screen === 'damaged'
              ? 'damaged'
              : screen === 'recovery'
                ? 'recovery'
                : screen === 'conflict'
                  ? 'conflict'
                  : 'empty',
      }),
      chooseFolder: async () => null,
      install: fakeRun,
      uninstall: fakeRun,
      restore: fakeRun,
      repair: fakeRun,
      scan: async () => ({
        damaged: screen === 'damaged' ? ['prism.exe'] : [],
        checked: screen === 'damaged' ? 2 : 4,
        total: 4,
        truncated: screen === 'damaged',
      }),
      readLog: async () => ({
        path: 'Preview only — no files are written',
        text: 'Preview fixture — this is not an actual installer run.\n[10:42:03] Setup opened\n[10:42:03] Target folder checked\n',
        truncated: false,
      }),
      openLogFolder: async () => {},
      finish: async () => {},
      onProgress: async (handler) => {
        listener = handler;
      },
      minimize: () => {},
      close: () => {},
    };
  }

  // ---------- Helpers ----------
  function formatBytes(bytes) {
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let value = bytes;
    let unit = 0;
    while (value >= 1000 && unit < units.length - 1) {
      value /= 1000;
      unit += 1;
    }
    return (unit >= 2 && value < 10 ? value.toFixed(1) : Math.round(value)) + ' ' + units[unit];
  }

  /** Shows how far along we are, both in the bar and in the hero art. */
  function setProgress(fraction) {
    if (!Number.isFinite(fraction)) {
      $('percent').textContent = '';
      $('track').removeAttribute('aria-valuenow');
      return;
    }
    const done = Math.max(0, Math.min(1, fraction));
    root.style.setProperty('--done', String(done));
    $('percent').textContent = Math.round(done * 100) + '%';
    $('track').setAttribute('aria-valuenow', String(Math.round(done * 100)));
  }

  // ---------- Terms of use ----------
  /** The install button works only with a usable folder and an accepted agreement. */
  function updateInstallButton() {
    $('install').disabled =
      backendUnavailable || !folderOk || folderPending || operationBusy || !$('agree').checked;
  }

  /** Fills the terms card once, in the setup language (English when there is no translation). */
  function renderTerms() {
    const terms = window.SETUP_TERMS[language] || window.SETUP_TERMS.en;
    $('terms-title').textContent = terms.title;
    const body = $('terms-body');
    body.replaceChildren();
    for (const [heading, paragraph] of terms.sections) {
      const h = document.createElement('h3');
      h.textContent = heading;
      const p = document.createElement('p');
      p.textContent = paragraph;
      body.append(h, p);
    }
  }

  function openTerms() {
    $('terms').hidden = false;
    root.classList.add('reading');
    updateModalInert();
    $('terms-body').scrollTop = 0;
    $('terms-body').focus();
  }

  function closeTerms() {
    $('terms').hidden = true;
    root.classList.remove('reading');
    updateModalInert();
    $('open-terms').focus();
  }

  /** Opens or folds the "Custom settings" card (install folder and shortcuts). */
  function setCustomizing(open) {
    root.classList.toggle('customizing', open);
    $('customize').setAttribute('aria-expanded', String(open));
    if (open && document.activeElement === $('customize')) $('folder').focus();
  }

  function show(screen) {
    // The custom-settings card belongs to the install screen only.
    if (screen !== 'options') setCustomizing(false);
    root.dataset.screen = screen;
    // While files are being written, closing the window would leave a half-done install.
    const busy = screen === 'progress';
    $('close').disabled = busy || backendUnavailable;
    $('progress-folder').textContent = $('folder').value || info?.installDir || '';
    const focusTarget = {
      // Until the terms are accepted the install button is disabled, so start on the checkbox.
      options: $('agree').checked ? 'install' : 'agree',
      uninstall: 'remove-data',
      restore: 'restore-ack',
      done: 'finish',
    }[screen];
    if (focusTarget) $(focusTarget).focus();
  }

  // ---------- Screens ----------
  let info = null;
  let folderOk = false;
  let folderPending = true;
  let folderEpoch = 0;
  let folderCheck = null;
  let folderScan = null;
  let scanEpoch = 0;
  let operationBusy = false;
  let currentStage = 'preparing';
  let resultCopy = null;
  let logEpoch = 0;
  let logTimer = 0;
  let logText = '';
  let detailTrigger = null;
  let previous = null;
  let previousPath = '';
  let previousFree = null;
  let restoreSelection = null;
  function invalidateRestoreSelection() {
    restoreSelection = null;
    $('restore-ack').checked = false;
    $('close-running-restore').checked = false;
    $('restore').disabled = true;
  }
  function updatePrevious(value, path, free) {
    invalidateRestoreSelection();
    previous = value;
    previousPath = path;
    previousFree = free;
    $('previous-options').hidden = !value;
    $('previous-uninstall').hidden = !value;
  }
  function showRestore() {
    if (
      !previous ||
      operationBusy ||
      (!['uninstall', 'restore'].includes(info.mode) && folderPending)
    )
      return;
    restoreSelection = { ...previous, path: previousPath, free: previousFree };
    $('restore-title').textContent = t('restoreTitle', {
      from: restoreSelection.currentVersion,
      to: restoreSelection.version,
    });
    $('restore-folder').textContent = restoreSelection.path;
    const enough =
      restoreSelection.free == null || restoreSelection.free >= restoreSelection.requiredBytes;
    $('restore-space').textContent =
      restoreSelection.free == null
        ? ''
        : t(enough ? 'space' : 'spaceShort', {
            need: formatBytes(restoreSelection.requiredBytes),
            free: formatBytes(restoreSelection.free),
          });
    $('restore-ack').checked = false;
    $('close-running-restore').checked = false;
    $('restore').disabled = true;
    $('restore-ack').disabled = !enough;
    show('restore');
  }
  async function runRestore() {
    if (
      backendUnavailable ||
      operationBusy ||
      !restoreSelection ||
      !$('restore-ack').checked ||
      $('restore-ack').disabled
    )
      return;
    const selection = restoreSelection;
    const target = selection.version;
    const closeRunningApp = $('close-running-restore').checked;
    beginOperation(selection.path);
    try {
      await backend.restore(selection.path, target, closeRunningApp);
      setProgress(1);
      $('done-title').textContent = t('restored');
      $('version').textContent = t('version', { v: target });
      $('done-detail').textContent = t('version', { v: target });
      $('done-detail').classList.remove('error');
      // Older code may not support newer data. Launch requires a fresh choice.
      $('launch').checked = false;
      $('launch-option').hidden = false;
      $('finish').textContent = t('finish');
      $('finish').onclick = null;
      resultCopy = {
        title: 'restored',
        detail: 'version',
        values: { v: target },
        finish: 'finish',
      };
      root.classList.remove('failed');
      $('done-symbol').textContent = '✓';
      show('done');
    } catch (error) {
      showFailure(t('restoreFailed'), error, 'restore');
    } finally {
      operationBusy = false;
    }
  }

  async function checkFolder() {
    const path = $('folder').value;
    const epoch = ++folderEpoch;
    folderPending = true;
    folderOk = false;
    folderScan = null;
    // A refresh may select another archive. Never keep consent for an old target.
    updatePrevious(null, '', null);
    renderFolderStatus();
    updateInstallButton();
    try {
      const check = await backend.checkFolder(path);
      if (epoch !== folderEpoch || $('folder').value !== path) return;
      folderPending = false;
      folderCheck = check;
      updatePrevious(
        check.recoveryPending ? null : check.previousVersion,
        path.trim(),
        check.freeBytes,
      );
      const free = check.freeBytes;
      const need = check.requiredBytes ?? info.requiredBytes;
      let message = '';
      folderOk = !check.error && check.health !== 'conflict';
      if (check.error || check.health === 'conflict') {
        message = check.error || t('folderConflict');
      } else if (free != null && free < need) {
        folderOk = false;
        message = t('spaceShort', { need: formatBytes(need), free: formatBytes(free) });
      } else if (free != null) {
        message = t('space', { need: formatBytes(need), free: formatBytes(free) });
      }
      if (folderOk && check.recoveryPending) message = t('recoveryPending') + ' ' + message;
      if (!info.payloadReady) {
        folderOk = false;
        message = t('noPayload');
      }
      if (check.previousError) message += ' ' + t('previousUnavailable');
      $('folder-hint').textContent = message;
      $('folder-hint').classList.toggle('error', !folderOk);
      // A folder problem can only be fixed in the custom-settings card, so open it.
      if (check.error && root.dataset.screen === 'options') setCustomizing(true);
      updateInstallButton();
      renderFolderStatus();
    } catch (error) {
      if (epoch !== folderEpoch || $('folder').value !== path) return;
      folderPending = false;
      folderOk = false;
      folderCheck = { error: String(error) };
      $('folder-hint').textContent = String(error);
      $('folder-hint').classList.add('error');
      renderFolderStatus();
      updateInstallButton();
    }
  }

  function sameInstallFolder() {
    return (
      info &&
      $('folder')
        .value.trim()
        .replace(/[\\/]+$/, '')
        .toLowerCase() ===
        String(info.installDir)
          .replace(/[\\/]+$/, '')
          .toLowerCase()
    );
  }
  function repairAvailable() {
    const available =
      typeof folderCheck?.repairAvailable === 'boolean'
        ? folderCheck.repairAvailable
        : sameInstallFolder() && info?.repairAvailable;
    return (
      available && !folderPending && !folderCheck?.recoveryPending && folderOk && !operationBusy
    );
  }
  function renderFolderStatus() {
    let state = 'ready';
    let label = 'folderReady';
    if (folderPending) {
      state = 'checking';
      label = 'checkingFolder';
    } else if (!folderOk || folderCheck?.error || folderCheck?.health === 'conflict') {
      state = 'conflict';
      label = 'folderConflict';
    } else if (folderCheck?.recoveryPending || folderCheck?.health === 'recovery') {
      state = 'recovery';
      label = 'folderRecovery';
    } else if (folderScan?.damaged.length || folderCheck?.health === 'damaged') {
      state = 'damaged';
      label = 'scanDamaged';
    } else if (
      folderCheck?.health === 'installed' ||
      (!folderCheck?.health && sameInstallFolder() && info?.installedVersion)
    ) {
      state = 'installed';
      label = 'folderInstalled';
    }
    $('location-state').dataset.state = state;
    $('location-state-text').textContent = folderScan
      ? t(folderScan.damaged.length ? 'scanDamaged' : 'scanClean', {
          count: folderScan.damaged.length || folderScan.checked,
        })
      : label === 'scanDamaged'
        ? t('folderDamaged')
        : t(label);
    $('repair').hidden = !repairAvailable();
    $('scan').hidden = !repairAvailable();
    const coverage =
      folderScan && Number.isFinite(folderScan.checked) && Number.isFinite(folderScan.total)
        ? t('scanCoverage', { checked: folderScan.checked, total: folderScan.total })
        : '';
    const partial = folderScan?.truncated ? t('scanPartial') : '';
    $('scan-detail').textContent = [coverage, partial].filter(Boolean).join(' · ');
    $('scan-detail').hidden = !coverage && !partial;
  }

  function beginOperation(path) {
    operationBusy = true;
    resultCopy = null;
    root.classList.remove('failed');
    $('done-detail').classList.remove('error');
    $('technical-error').hidden = true;
    $('technical-error-text').textContent = '';
    currentStage = 'preparing';
    $('stage').textContent = t('preparing');
    for (const operation of ['install', 'uninstall', 'repair', 'restore'])
      $('close-running-' + operation).checked = false;
    invalidateRestoreSelection();
    show('progress');
    if (path) $('progress-folder').textContent = path;
    setProgress(0);
  }
  async function runInstall() {
    if (backendUnavailable || operationBusy) return;
    const options = {
      installDir: $('folder').value.trim(),
      desktopShortcut: $('desktop').checked,
      startMenuShortcut: $('start-menu').checked,
    };
    const closeRunningApp = $('close-running-install').checked;
    beginOperation(options.installDir);
    try {
      await backend.install(options, closeRunningApp);
      setProgress(1);
      if (info.ui === 'passive') {
        // Started by the updater: close by itself and start Prism again if asked to.
        await backend.finish(info.relaunch);
        return;
      }
      $('done-title').textContent = t(info.mode === 'update' ? 'updated' : 'installed');
      $('done-detail').textContent = t('installedDetail');
      $('done-detail').classList.remove('error');
      $('launch-option').hidden = false;
      $('finish').textContent = t('finish');
      $('finish').onclick = null;
      resultCopy = {
        title: info.mode === 'update' ? 'updated' : 'installed',
        detail: 'installedDetail',
        finish: 'finish',
      };
      root.classList.remove('failed');
      $('done-symbol').textContent = '✓';
      show('done');
    } catch (error) {
      showFailure(t('failed'), error, info.ui === 'passive' ? null : 'options');
      if (info.ui === 'passive' && info.relaunch) {
        // Started by the updater, which closed Prism. The failed update was rolled back, so
        // the Close button starts the earlier version again (X and Esc only close).
        // Setup refuses the restart when that version is not intact; then it simply closes.
        $('finish').onclick = (event) => {
          event.preventDefault();
          backend.finish(true).catch(() => backend.finish(false));
        };
      }
    } finally {
      operationBusy = false;
    }
  }

  async function runUninstall() {
    if (backendUnavailable || operationBusy) return;
    const removeData = $('remove-data').checked;
    const closeRunningApp = $('close-running-uninstall').checked;
    beginOperation(info.installDir);
    try {
      await backend.uninstall(removeData, closeRunningApp);
      setProgress(1);
      $('done-title').textContent = t('uninstalled');
      $('done-detail').textContent = t(removeData ? 'removedData' : 'keptData');
      $('done-detail').classList.remove('error');
      $('launch-option').hidden = true;
      $('finish').textContent = t('close');
      $('finish').onclick = null;
      resultCopy = {
        title: 'uninstalled',
        detail: removeData ? 'removedData' : 'keptData',
        finish: 'close',
      };
      root.classList.remove('failed');
      $('done-symbol').textContent = '✓';
      show('done');
    } catch (error) {
      showFailure(t('uninstallFailed'), error, 'uninstall');
    } finally {
      operationBusy = false;
    }
  }

  const failureMessages = {
    APP_RUNNING: 'appRunning',
    APP_CLOSE_FAILED: 'appCloseFailed',
    FOREIGN_INSTALLATION: 'foreignInstallation',
  };
  /** Translate stable backend reasons while preserving the exact technical error in details. */
  function showFailure(title, error, retryScreen, retryAction = null) {
    const raw = String(error);
    const message = error instanceof Error ? error.message : raw;
    const code = /^(APP_RUNNING|APP_CLOSE_FAILED|FOREIGN_INSTALLATION)(?=:|\s|$)/u.exec(
      message,
    )?.[1];
    const detail = code ? failureMessages[code] : null;
    resultCopy = {
      title:
        title === t('restoreFailed')
          ? 'restoreFailed'
          : title === t('uninstallFailed')
            ? 'uninstallFailed'
            : 'failed',
      raw: detail ? undefined : raw,
      detail,
      technical: raw,
      finish: retryScreen ? 'tryAgain' : 'close',
    };
    root.classList.add('failed');
    $('done-symbol').textContent = '!';
    $('done-title').textContent = title;
    $('done-detail').textContent = detail ? t(detail) : raw;
    $('done-detail').classList.add('error');
    $('launch-option').hidden = true;
    $('finish').textContent = t(retryScreen ? 'tryAgain' : 'close');
    $('finish').onclick = retryScreen
      ? async (event) => {
          event.preventDefault();
          show(retryScreen);
          if (retryScreen === 'restore') showRestore();
          if (retryScreen === 'options') {
            await checkFolder();
            if (retryAction === 'repair') showRepair();
          }
        }
      : null;
    show('done');
  }

  function updateModalInert() {
    const open = [...document.querySelectorAll('.modal')].some((modal) => !modal.hidden);
    for (const element of document.querySelectorAll('.titlebar,.workspace,.hero'))
      element.inert = open;
  }
  async function refreshLog() {
    const epoch = ++logEpoch;
    clearTimeout(logTimer);
    $('refresh-log').disabled = true;
    try {
      const log = await backend.readLog();
      if (epoch !== logEpoch || $('details').hidden) return;
      logText = log.text || '';
      $('log-path').textContent = log.path || t('noLogFile');
      $('log-text').textContent = logText || t('logEmpty');
      $('log-scope').textContent = log.truncated ? t('logTail') : '';
      $('log-scope').hidden = !log.truncated;
      $('open-log-folder').disabled = !log.path;
      $('copy-log').disabled = !logText;
      $('log-notice').textContent = '';
      $('log-notice').classList.remove('error');
    } catch (error) {
      if (epoch !== logEpoch || $('details').hidden) return;
      $('log-notice').textContent = t('logUnavailable') + ' ' + String(error);
      $('log-notice').classList.add('error');
    } finally {
      if (epoch === logEpoch) {
        $('refresh-log').disabled = false;
        if (!$('details').hidden && operationBusy)
          logTimer = setTimeout(() => void refreshLog(), 1000);
      }
    }
  }
  function openDetails(trigger) {
    detailTrigger = trigger;
    $('detail-folder').textContent = $('folder').value || info?.installDir || '';
    $('detail-version').textContent = info?.version || '';
    $('technical-error').hidden = !resultCopy?.technical;
    $('technical-error-text').textContent = resultCopy?.technical || '';
    $('details').hidden = false;
    updateModalInert();
    $('details-close').focus();
    void refreshLog();
  }
  function closeDetails() {
    $('details').hidden = true;
    ++logEpoch;
    clearTimeout(logTimer);
    updateModalInert();
    if (detailTrigger?.getClientRects().length) detailTrigger.focus();
  }
  async function scanInstallation() {
    if (!repairAvailable()) return;
    const path = $('folder').value.trim();
    const epoch = ++scanEpoch;
    const checkEpoch = folderEpoch;
    $('scan').disabled = true;
    $('location-state-text').textContent = t('scanning');
    try {
      const scan = await backend.scan(path);
      if (epoch !== scanEpoch || checkEpoch !== folderEpoch || $('folder').value.trim() !== path)
        return;
      folderScan = scan;
      renderFolderStatus();
    } catch (error) {
      if (epoch === scanEpoch && checkEpoch === folderEpoch)
        $('folder-hint').textContent = String(error);
    } finally {
      if (epoch === scanEpoch) $('scan').disabled = false;
    }
  }
  function showRepair() {
    if (!repairAvailable()) return;
    if (!$('agree').checked) {
      openTerms();
      return;
    }
    $('repair-ack').checked = false;
    $('close-running-repair').checked = false;
    $('repair-confirm').disabled = true;
    $('repair-folder').textContent = $('folder').value.trim();
    $('repair-dialog').hidden = false;
    updateModalInert();
    $('repair-ack').focus();
  }
  function closeRepair() {
    $('repair-dialog').hidden = true;
    $('close-running-repair').checked = false;
    updateModalInert();
    $('repair').focus();
  }
  async function runRepair() {
    if (!repairAvailable() || !$('repair-ack').checked || !$('agree').checked) return;
    const path = $('repair-folder').textContent;
    if (path !== $('folder').value.trim()) return;
    const closeRunningApp = $('close-running-repair').checked;
    closeRepair();
    beginOperation(path);
    try {
      await backend.repair(path, closeRunningApp);
      setProgress(1);
      resultCopy = { title: 'repaired', detail: 'installedDetail', finish: 'finish' };
      $('done-title').textContent = t('repaired');
      $('done-detail').textContent = t('installedDetail');
      $('done-detail').classList.remove('error');
      root.classList.remove('failed');
      $('done-symbol').textContent = '✓';
      $('launch-option').hidden = false;
      $('launch').checked = false;
      $('finish').textContent = t('finish');
      $('finish').onclick = null;
      show('done');
    } catch (error) {
      showFailure(t('failed'), error, 'options', 'repair');
    } finally {
      operationBusy = false;
    }
  }

  const stageText = {
    closing: 'closing',
    preparing: 'preparing',
    recovering: 'recovering',
    copying: 'copying',
    shortcuts: 'shortcuts',
    registering: 'registering',
    removing: 'removing',
    done: 'finish',
  };

  function renderVersion() {
    if (!info) return;
    $('version').textContent =
      !info.recoveryPending &&
      info.mode === 'update' &&
      info.installedVersion &&
      info.installedVersion !== info.version
        ? t('updateFrom', { old: info.installedVersion, v: info.version })
        : t('version', { v: info.version });
    $('install').textContent = t(info.mode === 'update' ? 'update' : 'install');
    $('setup-heading').textContent = t(
      info.mode === 'uninstall' ? 'uninstall' : info.mode === 'restore' ? 'previous' : 'setupTitle',
    );
  }

  async function start() {
    renderTerms();
    info = await backend.info();
    await backend.onProgress(({ stage, fraction }) => {
      currentStage = stage;
      $('stage').textContent = t(stageText[stage] || 'preparing');
      setProgress(stage === 'closing' ? NaN : fraction);
    });
    $('stage').textContent = t('preparing');

    renderVersion();

    if (info.mode === 'uninstall') {
      updatePrevious(
        info.recoveryPending ? null : info.previousVersion,
        info.installDir,
        info.freeBytes,
      );
      $('uninstall-folder').textContent = info.installDir;
      if (info.previousError) $('uninstall-folder').textContent += ' · ' + t('previousUnavailable');
      show('uninstall');
      return;
    }
    if (info.mode === 'restore') {
      updatePrevious(
        info.recoveryPending ? null : info.previousVersion,
        info.installDir,
        info.freeBytes,
      );
      if (!previous) throw new Error(t('restoreFailed'));
      showRestore();
      return;
    }
    $('folder').value = info.installDir;
    $('desktop').checked = info.desktopShortcut;
    $('start-menu').checked = info.startMenuShortcut;
    $('install').textContent = t(info.mode === 'update' ? 'update' : 'install');
    root.classList.add('customizing');
    if (info.ui === 'passive') {
      // Started by Prism's updater: the user already agreed when installing.
      runInstall();
      return;
    }
    updateInstallButton();
    show('options');
    await checkFolder();
    if (preview?.get('preview') === 'done') {
      resultCopy = { title: 'installed', detail: 'installedDetail', finish: 'finish' };
      $('done-title').textContent = t('installed');
      $('done-detail').textContent = t('installedDetail');
      show('done');
    } else if (preview?.get('preview') === 'error')
      showFailure(t('failed'), 'Preview fixture: close Prism and retry setup.', 'options');
  }

  // ---------- Events ----------
  let typingTimer = 0;
  $('folder').addEventListener('input', () => {
    $('close-running-install').checked = false;
    clearTimeout(typingTimer);
    ++folderEpoch;
    ++scanEpoch;
    folderPending = true;
    folderOk = false;
    folderScan = null;
    updatePrevious(null, '', null);
    $('scan').disabled = false;
    renderFolderStatus();
    updateInstallButton();
    typingTimer = setTimeout(() => void checkFolder(), 250);
  });
  $('customize').addEventListener('click', () =>
    setCustomizing(!root.classList.contains('customizing')),
  );
  $('browse').addEventListener('click', async () => {
    const before = $('folder').value;
    const epoch = folderEpoch;
    $('browse').disabled = true;
    try {
      const picked = await backend.chooseFolder(before);
      if (epoch !== folderEpoch || $('folder').value !== before || operationBusy) return;
      if (picked) {
        $('close-running-install').checked = false;
        $('folder').value = picked;
        await checkFolder();
      }
    } catch (error) {
      $('folder-hint').textContent = String(error);
    } finally {
      $('browse').disabled = false;
    }
  });
  $('options-form').addEventListener('submit', (event) => {
    event.preventDefault();
    if (!folderPending && folderOk && $('agree').checked) void runInstall();
  });
  $('agree').addEventListener('change', updateInstallButton);
  $('open-terms').addEventListener('click', openTerms);
  $('terms-close').addEventListener('click', closeTerms);
  $('terms-agree').addEventListener('click', () => {
    $('agree').checked = true;
    updateInstallButton();
    closeTerms();
  });
  $('uninstall-form').addEventListener('submit', (event) => {
    event.preventDefault();
    runUninstall();
  });
  for (const id of ['previous-options', 'previous-uninstall'])
    $(id).addEventListener('click', showRestore);
  $('restore-back').addEventListener('click', () => {
    invalidateRestoreSelection();
    if (info.mode === 'restore') backend.finish(false);
    else show(info.mode === 'uninstall' ? 'uninstall' : 'options');
  });
  $('restore-ack').addEventListener('change', () => {
    $('restore').disabled =
      !restoreSelection || !$('restore-ack').checked || $('restore-ack').disabled;
  });
  $('restore-form').addEventListener('submit', (event) => {
    event.preventDefault();
    runRestore();
  });
  $('done-form').addEventListener('submit', (event) => {
    event.preventDefault();
    if (backendUnavailable) return;
    const launch = !$('launch-option').hidden && $('launch').checked;
    backend.finish(launch).catch((error) => showFailure(t('failed'), error, null));
  });
  $('minimize').addEventListener('click', () => backend.minimize());
  $('close').addEventListener('click', () => backend.close());
  $('language').addEventListener('change', () => {
    language = $('language').value;
    index = languages.indexOf(language);
    savePreference('language', language);
    renderLabels();
    renderTerms();
    renderVersion();
    $('stage').textContent = t(stageText[currentStage] || 'preparing');
    if (resultCopy) {
      $('done-title').textContent = t(resultCopy.title, resultCopy.values);
      $('done-detail').textContent = resultCopy.raw ?? t(resultCopy.detail, resultCopy.values);
      $('finish').textContent = t(resultCopy.finish);
    }
    if (restoreSelection)
      $('restore-title').textContent = t('restoreTitle', {
        from: restoreSelection.currentVersion,
        to: restoreSelection.version,
      });
    renderFolderStatus();
    if (info && root.dataset.screen === 'options') void checkFolder();
  });
  $('theme').addEventListener('change', () => {
    theme = $('theme').value;
    savePreference('theme', theme);
    applyTheme();
  });
  for (const trigger of document.querySelectorAll('.detail-trigger'))
    trigger.addEventListener('click', () => openDetails(trigger));
  $('details-close').addEventListener('click', closeDetails);
  $('refresh-log').addEventListener('click', () => void refreshLog());
  $('open-log-folder').addEventListener(
    'click',
    () =>
      void backend.openLogFolder().catch((error) => {
        $('log-notice').textContent = String(error);
      }),
  );
  $('copy-log').addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(logText);
      $('log-notice').textContent = t('logCopied');
    } catch {
      $('log-notice').textContent = t('copyFailed');
      const selection = getSelection();
      const range = document.createRange();
      range.selectNodeContents($('log-text'));
      selection.removeAllRanges();
      selection.addRange(range);
      $('log-text').focus();
    }
  });
  $('scan').addEventListener('click', () => void scanInstallation());
  $('repair').addEventListener('click', showRepair);
  $('repair-cancel').addEventListener('click', closeRepair);
  $('repair-ack').addEventListener('change', () => {
    $('repair-confirm').disabled = !$('repair-ack').checked;
  });
  $('repair-confirm').addEventListener('click', () => void runRepair());
  // Show focus rings once the keyboard is used (see .keyboard in setup.css); hide them again
  // when the mouse is used.
  document.addEventListener('keydown', () => root.classList.add('keyboard'), true);
  document.addEventListener('pointerdown', () => root.classList.remove('keyboard'), true);
  // Esc first folds the custom-settings card; otherwise it closes the window (never while
  // installing).
  document.addEventListener('keydown', (event) => {
    if (backendUnavailable) return;
    const modal = [...document.querySelectorAll('.modal')].find((element) => !element.hidden);
    if (event.key === 'Tab' && modal) {
      const focusable = [
        ...modal.querySelectorAll('button:not(:disabled),input:not(:disabled),[tabindex="0"]'),
      ].filter((element) => element.getClientRects().length);
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (
        event.shiftKey &&
        (document.activeElement === first || !modal.contains(document.activeElement))
      ) {
        event.preventDefault();
        last?.focus();
      } else if (
        !event.shiftKey &&
        (document.activeElement === last || !modal.contains(document.activeElement))
      ) {
        event.preventDefault();
        first?.focus();
      }
    }
    if (event.key !== 'Escape') return;
    if (!$('details').hidden) closeDetails();
    else if (!$('repair-dialog').hidden) closeRepair();
    else if (!$('terms').hidden) {
      closeTerms();
    } else if (root.classList.contains('customizing')) {
      setCustomizing(false);
      $('customize').focus();
    } else if (root.dataset.screen !== 'progress') {
      backend.close();
    }
  });

  start().catch((error) => {
    showFailure(t('failed'), error, null);
    if (backendUnavailable) {
      resultCopy = { title: 'failed', detail: 'bridgeUnavailable', finish: 'close' };
      $('done-detail').textContent = t('bridgeUnavailable');
      for (const element of root.querySelectorAll('button,input')) element.disabled = true;
    }
  });
})();
