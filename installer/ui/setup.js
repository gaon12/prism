// Behaviour of the setup window. Plain JavaScript (no build step) so the setup stays small.
//
// Flow:  loading → options (or uninstall) → progress → done
// - In "passive" mode (started by Prism's updater) the options screen is skipped.
// - Opened in a normal browser (no Tauri), a fake backend is used so the design can be
//   previewed: open index.html?preview=options|progress|done|uninstall.

(function () {
  'use strict';

  const tauri = window.__TAURI__;
  const $ = (id) => document.getElementById(id);
  const root = document.querySelector('.setup');

  // ---------- Language ----------
  const languages = ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'];
  function pickLanguage() {
    const wanted = (navigator.language || 'en').toLowerCase();
    if (wanted.startsWith('zh')) {
      // Traditional Chinese for Taiwan, Hong Kong and Macau; Simplified otherwise.
      return /tw|hk|mo|hant/.test(wanted) ? 'zh-TW' : 'zh-CN';
    }
    const base = wanted.slice(0, 2);
    return languages.includes(base) ? base : 'en';
  }
  const language = pickLanguage();
  document.documentElement.lang = language;
  const index = languages.indexOf(language);

  /** Text for `key` in the chosen language, with `{name}` placeholders filled from `values`. */
  function t(key, values) {
    const list = window.SETUP_MESSAGES[key];
    let text = (list && (list[index] || list[1])) || key;
    for (const [name, value] of Object.entries(values || {})) {
      text = text.replaceAll('{' + name + '}', value);
    }
    return text;
  }

  // Fill every element marked with data-text, and the tooltips of the window buttons.
  for (const element of document.querySelectorAll('[data-text]')) {
    element.textContent = t(element.dataset.text);
  }
  for (const element of document.querySelectorAll('[data-label]')) {
    element.title = t(element.dataset.label);
    element.setAttribute('aria-label', element.title);
  }

  // ---------- Backend ----------
  const backend = tauri ? realBackend() : previewBackend();

  function realBackend() {
    const invoke = tauri.core.invoke;
    const appWindow = tauri.window.getCurrentWindow();
    return {
      info: () => invoke('setup_info'),
      checkFolder: (path) => invoke('check_folder', { path }),
      chooseFolder: (current) => invoke('choose_folder', { current }),
      install: (options) => invoke('run_install', { options }),
      uninstall: (removeData) => invoke('run_uninstall', { removeData }),
      restore: (installDir, expectedVersion) =>
        invoke('run_restore', { installDir, expectedVersion }),
      finish: (launch) => invoke('finish', { launch }),
      onProgress: (handler) =>
        tauri.event.listen('setup://progress', (event) => handler(event.payload)),
      minimize: () => appWindow.minimize(),
      close: () => appWindow.close(),
    };
  }

  // Fake backend for designing in a browser. Never used inside the real setup.
  function previewBackend() {
    const screen = new URLSearchParams(location.search).get('preview') || 'options';
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
        mode: screen === 'uninstall' ? 'uninstall' : 'install',
        ui: screen === 'progress' ? 'passive' : 'full',
        version: '0.2.0',
        installedVersion: null,
        installDir: 'C:\\Users\\you\\AppData\\Local\\Programs\\Prism',
        defaultDir: 'C:\\Users\\you\\AppData\\Local\\Programs\\Prism',
        desktopShortcut: true,
        startMenuShortcut: true,
        requiredBytes: 14_200_000,
        freeBytes: 212_000_000_000,
        payloadReady: true,
        relaunch: false,
      }),
      checkFolder: async () => ({ error: null, freeBytes: 212_000_000_000 }),
      chooseFolder: async () => null,
      install: fakeRun,
      uninstall: fakeRun,
      restore: fakeRun,
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
    const done = Math.max(0, Math.min(1, fraction));
    root.style.setProperty('--done', String(done));
    $('percent').textContent = Math.round(done * 100) + '%';
    $('track').setAttribute('aria-valuenow', String(Math.round(done * 100)));
  }

  function show(screen) {
    root.dataset.screen = screen;
    // While files are being written, closing the window would leave a half-done install.
    const busy = screen === 'progress';
    $('close').disabled = busy;
    const focusTarget = {
      options: 'install',
      uninstall: 'remove-data',
      restore: 'restore-ack',
      done: 'finish',
    }[screen];
    if (focusTarget) $(focusTarget).focus();
  }

  // ---------- Screens ----------
  let info = null;
  let folderOk = true;
  let previous = null;
  let previousPath = '';
  let previousFree = null;
  function updatePrevious(value, path, free) {
    previous = value;
    previousPath = path;
    previousFree = free;
    $('previous-options').hidden = !value;
    $('previous-uninstall').hidden = !value;
  }
  function showRestore() {
    if (!previous) return;
    $('restore-title').textContent = t('restoreTitle', {
      from: previous.currentVersion,
      to: previous.version,
    });
    const enough = previousFree == null || previousFree >= previous.requiredBytes;
    $('restore-space').textContent =
      previousFree == null
        ? ''
        : t(enough ? 'space' : 'spaceShort', {
            need: formatBytes(previous.requiredBytes),
            free: formatBytes(previousFree),
          });
    $('restore-ack').checked = false;
    $('restore').disabled = true;
    $('restore-ack').disabled = !enough;
    show('restore');
  }
  async function runRestore() {
    if (!previous || !$('restore-ack').checked) return;
    const target = previous.version;
    show('progress');
    setProgress(0);
    try {
      await backend.restore(previousPath, target);
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
      show('done');
    } catch (error) {
      showFailure(t('restoreFailed'), error, 'restore');
    }
  }

  async function checkFolder() {
    const path = $('folder').value;
    const hint = $('folder-hint');
    const check = await backend.checkFolder(path);
    if ($('folder').value !== path) return;
    updatePrevious(
      check.recoveryPending ? null : check.previousVersion,
      path.trim(),
      check.freeBytes,
    );
    const free = check.freeBytes;
    const need = check.requiredBytes ?? info.requiredBytes;
    let message = '';
    folderOk = !check.error;
    if (check.error) {
      message = check.error;
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
    hint.textContent = message;
    hint.classList.toggle('error', !folderOk);
    $('install').disabled = !folderOk;
  }

  async function runInstall() {
    const options = {
      installDir: $('folder').value.trim(),
      desktopShortcut: $('desktop').checked,
      startMenuShortcut: $('start-menu').checked,
    };
    show('progress');
    setProgress(0);
    try {
      await backend.install(options);
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
      show('done');
    } catch (error) {
      showFailure(t('failed'), error, info.ui === 'passive' ? null : 'options');
    }
  }

  async function runUninstall() {
    const removeData = $('remove-data').checked;
    show('progress');
    setProgress(0);
    try {
      await backend.uninstall(removeData);
      setProgress(1);
      $('done-title').textContent = t('uninstalled');
      $('done-detail').textContent = t(removeData ? 'removedData' : 'keptData');
      $('done-detail').classList.remove('error');
      $('launch-option').hidden = true;
      $('finish').textContent = t('close');
      $('finish').onclick = null;
      show('done');
    } catch (error) {
      showFailure(t('uninstallFailed'), error, 'uninstall');
    }
  }

  /** Error screen. The message from Rust already says what to do (e.g. "Close Prism…"). */
  function showFailure(title, error, retryScreen) {
    $('done-title').textContent = title;
    $('done-detail').textContent = String(error);
    $('done-detail').classList.add('error');
    $('launch-option').hidden = true;
    $('finish').textContent = t(retryScreen ? 'tryAgain' : 'close');
    $('finish').onclick = retryScreen
      ? (event) => {
          event.preventDefault();
          show(retryScreen);
        }
      : null;
    show('done');
  }

  const stageText = {
    preparing: 'preparing',
    recovering: 'recovering',
    copying: 'copying',
    shortcuts: 'shortcuts',
    registering: 'registering',
    removing: 'removing',
    done: 'registering',
  };

  async function start() {
    info = await backend.info();
    await backend.onProgress(({ stage, fraction }) => {
      $('stage').textContent = t(stageText[stage] || 'preparing');
      setProgress(fraction);
    });
    $('stage').textContent = t('preparing');

    $('version').textContent =
      !info.recoveryPending &&
      info.mode === 'update' &&
      info.installedVersion &&
      info.installedVersion !== info.version
        ? t('updateFrom', { old: info.installedVersion, v: info.version })
        : t('version', { v: info.version });

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
    if (info.ui === 'passive') {
      runInstall();
      return;
    }
    show('options');
    checkFolder();
  }

  // ---------- Events ----------
  let typingTimer = 0;
  $('folder').addEventListener('input', () => {
    clearTimeout(typingTimer);
    typingTimer = setTimeout(checkFolder, 250);
  });
  $('browse').addEventListener('click', async () => {
    const picked = await backend.chooseFolder($('folder').value);
    if (picked) {
      $('folder').value = picked;
      checkFolder();
    }
  });
  $('options-form').addEventListener('submit', (event) => {
    event.preventDefault();
    if (folderOk) runInstall();
  });
  $('uninstall-form').addEventListener('submit', (event) => {
    event.preventDefault();
    runUninstall();
  });
  for (const id of ['previous-options', 'previous-uninstall'])
    $(id).addEventListener('click', showRestore);
  $('restore-back').addEventListener('click', () => {
    if (info.mode === 'restore') backend.finish(false);
    else show(info.mode === 'uninstall' ? 'uninstall' : 'options');
  });
  $('restore-ack').addEventListener('change', () => {
    $('restore').disabled = !$('restore-ack').checked;
  });
  $('restore-form').addEventListener('submit', (event) => {
    event.preventDefault();
    runRestore();
  });
  $('done-form').addEventListener('submit', (event) => {
    event.preventDefault();
    const launch = !$('launch-option').hidden && $('launch').checked;
    backend.finish(launch).catch((error) => showFailure(t('failed'), error, null));
  });
  $('minimize').addEventListener('click', () => backend.minimize());
  $('close').addEventListener('click', () => backend.close());
  // Esc closes the window, except while installing.
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && root.dataset.screen !== 'progress') backend.close();
  });

  start().catch((error) => showFailure(t('failed'), error, null));
})();
