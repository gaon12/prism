// Real HKCU/known-folder integration, exclusively on disposable GitHub-hosted Windows x64.
// Never run qualification on local/self-hosted computers. --self-test uses only a fresh temp fixture.
// Browser plugin unavailable: use the repository's Playwright/WebView2 CDP workflow.
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { execFileSync, spawn } from 'node:child_process';
import { lstat, mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve, win32 } from 'node:path';
import { fileURLToPath } from 'node:url';
import { until, untilWebView } from './harness.mjs';
import { verifyInstalledPayload } from './setup-payload.mjs';

const marker = '.prism-installed-release-owner';
// WScript.Shell.TargetPath can expose an ANSI spelling ("??") on English Windows.
// Explicit wide COM reads preserve the stored Unicode path without resolving/launching it.
// https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-ishelllinkw-getpath
const wideShortcutReader = `
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;
[ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IPrismWideShellLink {
  [PreserveSig] int GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count, IntPtr data, uint flags);
  [PreserveSig] int GetIDList(out IntPtr list);
  [PreserveSig] int SetIDList(IntPtr list);
  [PreserveSig] int GetDescription([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder text, int count);
  [PreserveSig] int SetDescription([MarshalAs(UnmanagedType.LPWStr)] string text);
  [PreserveSig] int GetWorkingDirectory([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count);
  [PreserveSig] int SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string path);
  [PreserveSig] int GetArguments([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder text, int count);
  [PreserveSig] int SetArguments([MarshalAs(UnmanagedType.LPWStr)] string text);
  [PreserveSig] int GetHotkey(out short value);
  [PreserveSig] int SetHotkey(short value);
  [PreserveSig] int GetShowCmd(out int value);
  [PreserveSig] int SetShowCmd(int value);
  [PreserveSig] int GetIconLocation([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count, out int index);
  [PreserveSig] int SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string path, int index);
  [PreserveSig] int SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string path, uint reserved);
  [PreserveSig] int Resolve(IntPtr window, uint flags);
  [PreserveSig] int SetPath([MarshalAs(UnmanagedType.LPWStr)] string path);
}
public sealed class PrismShortcutInfo {
  public string Target { get; set; }
  public string WorkingDirectory { get; set; }
  public string Arguments { get; set; }
}
public static class PrismWideShortcutReader {
  public const int BufferCharacters = 32768;
  private static string Bounded(StringBuilder value) {
    if (value.Length >= BufferCharacters - 1) throw new InvalidOperationException("Shortcut field may be truncated.");
    return value.ToString();
  }
  private static void Input(string value) {
    if (value == null || value.Length >= BufferCharacters - 1 || value.IndexOf((char)0) >= 0)
      throw new ArgumentException("Invalid or oversized fixture field.");
  }
  public static PrismShortcutInfo Read(string filename) {
    object link = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("00021401-0000-0000-C000-000000000046")));
    try {
      ((IPersistFile)link).Load(filename, 0);
      var path = new StringBuilder(BufferCharacters);
      int status = ((IPrismWideShellLink)link).GetPath(path, path.Capacity, IntPtr.Zero, 4);
      Marshal.ThrowExceptionForHR(status);
      if (status != 0 || path.Length == 0) throw new InvalidOperationException("Shortcut has no stored Unicode target.");
      var working = new StringBuilder(BufferCharacters);
      Marshal.ThrowExceptionForHR(((IPrismWideShellLink)link).GetWorkingDirectory(working, working.Capacity));
      var arguments = new StringBuilder(BufferCharacters);
      Marshal.ThrowExceptionForHR(((IPrismWideShellLink)link).GetArguments(arguments, arguments.Capacity));
      return new PrismShortcutInfo { Target = Bounded(path), WorkingDirectory = Bounded(working), Arguments = Bounded(arguments) };
    } finally {
      if (link != null) Marshal.FinalReleaseComObject(link);
    }
  }
  // Used only by --self-test with its newly created, marked OS-temp directory.
  public static void CreateFixture(string filename, string target, string working, string arguments) {
    Input(filename); Input(target); Input(working); Input(arguments);
    object link = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("00021401-0000-0000-C000-000000000046")));
    try {
      var wide = (IPrismWideShellLink)link;
      Marshal.ThrowExceptionForHR(wide.SetPath(target));
      Marshal.ThrowExceptionForHR(wide.SetWorkingDirectory(working));
      Marshal.ThrowExceptionForHR(wide.SetArguments(arguments));
      ((IPersistFile)link).Save(filename, true);
    } finally {
      if (link != null) Marshal.FinalReleaseComObject(link);
    }
  }
}
`;
const pathKey = (path) => win32.normalize(path.replace(/^\\\\\?\\/, '')).toLowerCase();
export function requireHostedWindows(platform, arch, env) {
  assert.equal(platform, 'win32', 'Requires a disposable GitHub-hosted Windows runner');
  assert.equal(arch, 'x64');
  for (const [key, value] of Object.entries({
    GITHUB_ACTIONS: 'true',
    RUNNER_ENVIRONMENT: 'github-hosted',
    RUNNER_OS: 'Windows',
    RUNNER_ARCH: 'X64',
    PRISM_INSTALLED_RELEASE_QUALIFICATION: '1',
  }))
    assert.equal(env[key], value, `Required hosted-runner guard: ${key}`);
  for (const key of ['RUNNER_TEMP', 'GITHUB_WORKSPACE', 'APPDATA', 'LOCALAPPDATA'])
    assert(env[key] && win32.isAbsolute(env[key]), `Missing absolute runner path: ${key}`);
  assert(/^\d+$/.test(env.GITHUB_RUN_ID ?? ''), 'Requires a workflow run id');
  for (const key of Object.keys(env))
    assert(
      !key.toUpperCase().startsWith('PRISM_DEV_') && key.toUpperCase() !== 'PRISM_SETUP_SANDBOX',
      'Debug/sandbox overrides are forbidden for installed-release qualification',
    );
}

export function assertOwnedPath(path, allowed, nonce) {
  assert(/^[0-9a-f]{32}$/.test(nonce), 'Invalid ownership nonce');
  assert(win32.isAbsolute(path) && !path.split(/[\\/]/).includes('..'), 'Unsafe owned path');
  assert(
    allowed.some((entry) => pathKey(entry) === pathKey(path)),
    'Path is not exactly owned',
  );
}

export function verifyPristineSystem(state) {
  assert.equal(state.registration, null, 'Existing Prism HKCU installation');
  assert.equal(state.machine, false, 'Existing machine Prism installation');
  assert.equal(state.startup, null, 'Existing Prism startup entry');
  assert.equal(state.approved, null, 'Existing Prism startup approval');
  assert.equal(state.machineStartup, null, 'Existing machine Prism startup entry');
  assert.equal(state.machineApproved, null, 'Existing machine Prism startup approval');
  assert.equal(state.processes.length, 0, 'Existing Prism process');
}

export function verifyIdentity(runtime, settings, nonce, sources) {
  assert.equal(runtime.native, true);
  assert.equal(runtime.platform, 'windows');
  assert.equal(runtime.development, false, 'A debug app cannot qualify a release');
  assert.equal(
    runtime.installKind,
    'installed',
    'A portable/isolated app cannot qualify installation',
  );
  assert.equal(runtime.startupError, null);
  assert.equal(settings.scopes.length, 1);
  assert.equal(settings.scopes[0].id, `installed_release_${nonce}`, 'Foreign/stale CDP profile');
  assert.equal(pathKey(settings.scopes[0].path), pathKey(sources));
  assert.equal(settings.scopes[0].filenameEnabled, false);
  assert.equal(settings.scopes[0].contentEnabled, false);
  assert.equal(settings.scopes[0].externalAllowed, false);
  assert.equal(settings.startOnLogin, false);
  assert.equal(settings.clipboard.enabled, false);
  assert.equal(settings.updates.autoCheck, false);
  assert.equal(settings.ocr.enabled, false);
  assert.equal(settings.automaticSemantic, false);
  assert.equal(settings.generation.local, null);
  assert.equal(settings.generation.external, null);
  assert.equal(settings.generation.allowExternalQuestions, false);
  assert.equal(settings.generation.fallback, false);
  assert.equal(settings.mcp.enabled, false);
  assert.equal(settings.mcp.servers.length, 0);
}

async function exists(path) {
  try {
    await lstat(path);
    return true;
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}
async function noLinks(path) {
  for (let current = resolve(path); ; current = dirname(current)) {
    if (await exists(current))
      assert(!(await lstat(current)).isSymbolicLink(), 'Linked ownership path');
    if (dirname(current) === current) return;
  }
}
async function hash(path) {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
}
function powershell(script, extra = {}) {
  return execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $ErrorActionPreference='Stop'; " +
        script,
    ],
    {
      windowsHide: true,
      encoding: 'utf8',
      timeout: 30_000,
      env: { ...process.env, ...extra },
    },
  ).trim();
}
function systemState() {
  return JSON.parse(
    powershell(`
    function Value($path) { if (Test-Path -LiteralPath $path) { ,((Get-Item -LiteralPath $path).GetValue('Prism', $null)) } else { $null } }
    $key='HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prism'
    $registration=$null
    if(Test-Path -LiteralPath $key) { $registration=Get-ItemProperty -LiteralPath $key | Select-Object DisplayName,DisplayVersion,InstallLocation,UninstallString,QuietUninstallString,EstimatedSize }
    @{
      registration=$registration
      machine=(Test-Path -LiteralPath 'HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prism') -or (Test-Path -LiteralPath 'HKLM:\\Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prism')
      startup=(Value 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run')
      approved=(Value 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run')
      machineStartup=(Value 'HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run')
      machineApproved=(Value 'HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run')
      roaming=[Environment]::GetFolderPath('ApplicationData')
      local=[Environment]::GetFolderPath('LocalApplicationData')
      desktop=[Environment]::GetFolderPath('DesktopDirectory')
      programs=[Environment]::GetFolderPath('Programs')
      commonDesktop=[Environment]::GetFolderPath('CommonDesktopDirectory')
      commonPrograms=[Environment]::GetFolderPath('CommonPrograms')
      processes=@(Get-CimInstance Win32_Process -Filter "Name='prism.exe' OR Name='prism-setup.exe'" | Select-Object ProcessId,ExecutablePath,CommandLine)
    } | ConvertTo-Json -Depth 5 -Compress
  `),
  );
}
async function freePort(port) {
  assert(Number.isInteger(port) && port > 1024 && port < 65536, 'Explicit app CDP port required');
  const reservation = createServer();
  await new Promise((done, reject) => {
    reservation.once('error', reject);
    reservation.listen(port, '127.0.0.1', done);
  });
  await new Promise((done) => reservation.close(done));
}
async function releasedPort(port) {
  await until(
    async () => {
      try {
        await freePort(port);
        return true;
      } catch (error) {
        if (error.code === 'EADDRINUSE') return false;
        throw error;
      }
    },
    'previous owned WebView releases its debug port',
    20_000,
  );
}

async function qualify() {
  requireHostedWindows(process.platform, process.arch, process.env); // Before any filesystem/OS mutation.
  const workspace = resolve(process.env.GITHUB_WORKSPACE);
  assert.equal(
    pathKey(resolve(dirname(fileURLToPath(import.meta.url)), '../..')),
    pathKey(workspace),
  );
  const setup = resolve(process.env.PRISM_SETUP_EXE ?? '');
  const expectedApp = resolve(process.env.PRISM_SETUP_EXPECTED_EXE ?? '');
  assert.equal(
    pathKey(setup),
    pathKey(join(workspace, 'installer/src-tauri/target/release/prism-setup.exe')),
  );
  assert.equal(
    pathKey(expectedApp),
    pathKey(join(workspace, 'src-tauri/target/release/prism.exe')),
  );
  for (const path of [setup, expectedApp]) {
    await noLinks(path);
    assert((await lstat(path)).isFile(), 'Release inputs must be regular files');
  }
  assert(
    /^[0-9a-f]{64}$/i.test(process.env.PRISM_TEST_APP_SHA256 ?? ''),
    'Independent app SHA-256 required',
  );
  const appHash = await hash(expectedApp);
  assert.equal(appHash, process.env.PRISM_TEST_APP_SHA256.toLowerCase());
  const setupHash = await hash(setup);
  const { version } = JSON.parse(
    await readFile(join(workspace, 'src-tauri/tauri.conf.json'), 'utf8'),
  );
  const before = systemState();
  verifyPristineSystem(before);
  assert.equal(pathKey(before.roaming), pathKey(process.env.APPDATA));
  assert.equal(pathKey(before.local), pathKey(process.env.LOCALAPPDATA));
  const profiles = [
    join(before.roaming, 'app.prism.desktop'),
    join(before.local, 'app.prism.desktop'),
  ];
  const shortcuts = [join(before.desktop, 'Prism.lnk'), join(before.programs, 'Prism.lnk')];
  for (const path of [
    ...profiles,
    ...shortcuts,
    join(before.commonDesktop, 'Prism.lnk'),
    join(before.commonPrograms, 'Prism.lnk'),
    join(before.local, 'Programs/Prism'),
  ]) {
    await noLinks(path);
    assert(
      !(await exists(path)),
      'Existing Prism profile/install/shortcut; refusing qualification',
    );
  }
  const port = Number(process.env.PRISM_APP_CDP_PORT);
  await freePort(port);
  const nonce = randomBytes(16).toString('hex');
  const token = `--prism-installed-release-${nonce}`;
  const output = join(process.env.RUNNER_TEMP, `prism-installed-release-${nonce}`);
  // Keep ownership in a parent: first installation must receive an empty/new folder.
  const installationRoot = join(before.local, 'Programs', `Prism qualification 한글 ${nonce}`);
  const installed = join(installationRoot, 'app');
  const owned = [...profiles, output, installationRoot];
  for (const path of owned) {
    await noLinks(path);
    assert(!(await exists(path)), 'Owned fixture already exists');
  }
  const sources = join(output, 'sources');
  const environment = {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  };
  delete environment.WEBVIEW2_USER_DATA_FOLDER;
  const active = new Set();
  let instance;
  let failure;
  let installationAttempted = false;
  const report = { nonce, appHash, setupHash, version, checks: [], passed: false };
  const created = new Set();
  async function own(path) {
    assertOwnedPath(path, owned, nonce);
    await mkdir(path); // Exclusive: never adopt an existing directory.
    created.add(path);
    await writeFile(join(path, marker), nonce, { flag: 'wx' });
  }
  async function checkOwner(path) {
    assertOwnedPath(path, owned, nonce);
    await noLinks(path);
    const sentinel = join(path, marker);
    const metadata = await lstat(sentinel);
    assert(
      metadata.isFile() && metadata.nlink === 1,
      'Ownership marker must be a private regular file',
    );
    assert.equal(await readFile(sentinel, 'utf8'), nonce);
  }
  function start(path, args) {
    const child = spawn(path, args, { windowsHide: true, stdio: 'ignore', env: environment });
    child.on('error', (error) => {
      child.spawnError = error;
    });
    active.add(child);
    return child;
  }
  async function exit(child, expectedCode = 0) {
    await until(
      () => {
        if (child.spawnError) throw child.spawnError;
        return child.exitCode !== null || child.signalCode !== null;
      },
      'owned release setup process exit',
      180_000,
    );
    active.delete(child);
    assert.equal(child.exitCode, expectedCode);
  }
  function ownedAppProcesses() {
    return systemState().processes.filter(
      (item) =>
        item.ExecutablePath &&
        pathKey(item.ExecutablePath) === pathKey(join(installed, 'prism.exe')) &&
        item.CommandLine?.includes(token),
    );
  }
  async function checkInstallation() {
    await checkOwner(installationRoot);
    await noLinks(installed);
    const registration = systemState().registration;
    assert(registration, 'Real HKCU registration missing');
    assert.equal(registration.DisplayName, 'Prism');
    assert.equal(registration.DisplayVersion, version);
    assert.equal(pathKey(registration.InstallLocation), pathKey(installed));
    assert.equal(registration.UninstallString, `"${join(installed, 'uninstall.exe')}"`);
    assert.equal(registration.QuietUninstallString, `"${join(installed, 'uninstall.exe')}" /S`);
    assert(registration.EstimatedSize > 0);
    await verifyInstalledPayload(installed, expectedApp);
    assert.equal(await hash(join(installed, 'uninstall.exe')), setupHash);
    for (const shortcut of shortcuts) {
      await noLinks(shortcut);
      const info = await lstat(shortcut);
      assert(info.isFile() && info.size > 0 && info.size <= 1024 * 1024, 'Invalid owned shortcut');
    }
    const targets = JSON.parse(
      powershell(
        `
      Add-Type -TypeDefinition @'
${wideShortcutReader}
'@
      $targets=@(foreach($link in (ConvertFrom-Json -InputObject $env:PRISM_QUAL_LINKS)) { [PrismWideShortcutReader]::Read($link) })
      ConvertTo-Json -InputObject $targets -Compress
    `,
        { PRISM_QUAL_LINKS: JSON.stringify(shortcuts) },
      ),
    );
    report.shortcutTargets = targets;
    assert.equal(targets.length, 2);
    for (const target of targets) {
      assert.equal(pathKey(target.Target), pathKey(join(installed, 'prism.exe')));
      assert.equal(pathKey(target.WorkingDirectory), pathKey(installed));
      assert.equal(target.Arguments, '');
    }
    assert.equal(systemState().startup, null, 'Disabled auto-start must stay disabled');
  }
  async function attach(child) {
    await untilWebView(child ?? { exitCode: null, pid: null }, port, 'installed release WebView');
    const { chromium } = await import('@playwright/test');
    const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    instance = { browser, verified: false };
    let page;
    await until(
      () => {
        page = browser
          .contexts()
          .flatMap((context) => context.pages())
          .find(
            (candidate) =>
              candidate.url().includes('tauri.localhost') && !candidate.url().includes('#quick'),
          );
        return page;
      },
      'installed release main window',
      90_000,
    );
    const invoke = (command, args = {}) =>
      page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
        command,
        args,
      });
    // Only identity reads until the expected nonce and installed release mode both match.
    const runtime = await invoke('runtime_info');
    const settings = await invoke('get_settings');
    verifyIdentity(runtime, settings, nonce, sources);
    assert.equal(ownedAppProcesses().length, 1, 'Installed executable/nonce process identity');
    instance = { browser, page, invoke, verified: true };
    await page.getByRole('heading', { name: '파일 검색', exact: true }).waitFor();
    await page.screenshot({ path: join(output, `installed-${report.checks.length}.png`) });
    report.runtime = runtime;
  }
  async function stopApp() {
    if (!instance) return;
    if (instance.verified) {
      await instance.invoke('quit_app').catch((error) => {
        assert.match(String(error), /closed|Target|context|destroyed/i);
      });
      await until(() => ownedAppProcesses().length === 0, 'owned installed app shutdown', 30_000);
    }
    await instance.browser.close().catch(() => {});
    instance = null;
  }
  async function uninstallOwned() {
    await checkOwner(installationRoot);
    await noLinks(installed);
    const state = systemState();
    if (state.registration)
      assert.equal(pathKey(state.registration.InstallLocation), pathKey(installed));
    assert.equal(ownedAppProcesses().length, 0, 'Do not uninstall a running app');
    const uninstaller = join(installed, 'uninstall.exe');
    if (!(await exists(uninstaller))) return;
    assert.equal(await hash(uninstaller), setupHash, 'Only execute the owned uninstaller bytes');
    await exit(start(uninstaller, ['/S']));
    // Bootstrap exit precedes its owned temporary uninstaller's actual completion.
    await until(
      async () =>
        !(await exists(join(installed, 'install.json'))) &&
        !(await exists(uninstaller)) &&
        systemState().registration === null,
      'real silent uninstall completion',
      180_000,
    );
  }
  try {
    // Parents already exist on the hosted runner. Do not recursively create/adopt profile roots.
    for (const path of [output, ...profiles]) await own(path);
    await mkdir(join(before.local, 'Programs'), { recursive: true });
    await own(installationRoot);
    await mkdir(sources);
    await writeFile(
      join(profiles[0], 'settings.json'),
      JSON.stringify({
        onboardingVersion: 1,
        language: 'ko',
        theme: 'light',
        startOnLogin: false,
        shortcut: 'Ctrl+Alt+Shift+F9',
        automaticSemantic: false,
        closeToTray: true,
        clipboard: { enabled: false, onboarded: true },
        updates: { autoCheck: false },
        ocr: { enabled: false },
        mcp: { enabled: false, servers: [] },
        generation: { allowExternalQuestions: false, fallback: false, local: null, external: null },
        scopes: [
          {
            id: `installed_release_${nonce}`,
            name: nonce,
            path: sources,
            filenameEnabled: false,
            contentEnabled: false,
            externalAllowed: false,
          },
        ],
      }),
      { flag: 'wx' },
    );
    const settingsBefore = await readFile(join(profiles[0], 'settings.json'));
    installationAttempted = true;
    await exit(start(setup, ['/S', `/D=${installed}`]));
    await checkInstallation();
    await writeFile(join(installed, 'personal.txt'), nonce, { flag: 'wx' });
    report.checks.push('actual-hkcu-shortcuts-and-complete-release-payload');
    await freePort(port);
    const child = start(join(installed, 'prism.exe'), [token]);
    await attach(child);
    await instance.invoke('window_action', { action: 'close' });
    assert(
      ownedAppProcesses().length > 0,
      'Closing the window must leave the tray process running',
    );
    const recordWhileRunning = await readFile(join(installed, 'install.json'));
    await exit(start(setup, ['/S', '/UPDATE', `/D=${installed}`]), 1);
    assert(
      ownedAppProcesses().length > 0,
      'Silent setup must not close a running app without consent',
    );
    assert.deepEqual(await readFile(join(installed, 'install.json')), recordWhileRunning);
    assert.equal(await hash(join(installed, 'prism.exe')), appHash);
    await checkInstallation();
    report.checks.push('running-tray-app-refuses-silent-update-without-consent');
    await stopApp();
    await exit(child);
    report.checks.push('installed-release-launch-and-nonce-identity');
    await releasedPort(port);
    await exit(start(setup, ['/S', '/UPDATE', '/R', `/D=${installed}`, '/ARGS', token]));
    await attach(null);
    await checkInstallation();
    await stopApp();
    assert.deepEqual(await readFile(join(profiles[0], 'settings.json')), settingsBefore);
    assert.equal(await readFile(join(installed, 'personal.txt'), 'utf8'), nonce);
    report.checks.push('silent-update-relaunch-and-data-retention');
    await uninstallOwned();
    for (const shortcut of shortcuts) assert(!(await exists(shortcut)), 'Owned shortcut remains');
    assert.deepEqual(await readFile(join(profiles[0], 'settings.json')), settingsBefore);
    for (const path of profiles) await checkOwner(path);
    assert.equal(await readFile(join(installed, 'personal.txt'), 'utf8'), nonce);
    assert.equal(systemState().startup, null);
    assert(!(await exists(join(before.local, 'Programs/Prism'))));
    assert.equal(await hash(expectedApp), appHash);
    assert.equal(await hash(setup), setupHash);
    report.checks.push('real-uninstall-keeps-profile-and-unrecorded-user-files');
    report.passed = true;
  } catch (error) {
    failure = error;
    report.failure = String(error);
  } finally {
    const cleanupErrors = [];
    const attempt = async (action) => {
      try {
        await action();
      } catch (error) {
        cleanupErrors.push(String(error));
      }
    };
    // An unverified CDP page must never receive quit/model/settings IPC or screenshots.
    await attempt(stopApp);
    await attempt(async () => {
      for (const child of active) {
        if (child.exitCode === null && child.signalCode === null) child.kill();
        await until(
          () => child.exitCode !== null || child.signalCode !== null || child.spawnError,
          'owned child termination',
          20_000,
        );
      }
    });
    // Only an exact executable path plus this run's random process argument authorizes cleanup.
    await attempt(async () => {
      for (const process of ownedAppProcesses())
        powershell(
          `
      $p=Get-CimInstance Win32_Process -Filter "ProcessId=$env:PRISM_QUAL_PID"
      if($p -and $p.ExecutablePath -eq $env:PRISM_QUAL_APP -and $p.CommandLine.Contains($env:PRISM_QUAL_TOKEN)) { Stop-Process -Id $p.ProcessId -Force }
    `,
          {
            PRISM_QUAL_PID: String(process.ProcessId),
            PRISM_QUAL_APP: join(installed, 'prism.exe'),
            PRISM_QUAL_TOKEN: token,
          },
        );
      await until(() => ownedAppProcesses().length === 0, 'nonce-owned app termination', 20_000);
    });
    await attempt(async () => {
      if (installationAttempted && created.has(installationRoot)) await uninstallOwned();
      // Do not remove data while an uninstall/other operation is unfinished.
      assert.equal(systemState().registration, null, 'Registration remains after cleanup');
      assert(!(await exists(join(installed, 'prism.exe'))));
      assert(!(await exists(join(installed, 'uninstall.exe'))));
      // Keep reports/screenshots for CI; remove only exact marked fixture roots.
      for (const path of [installationRoot, ...profiles]) {
        if (!created.has(path)) continue;
        await checkOwner(path);
        await rm(path, { recursive: true });
      }
    });
    report.cleanupErrors = cleanupErrors;
    if (cleanupErrors.length) {
      report.passed = false;
      failure ??= new Error(
        'Installed-release qualification cleanup failed; owned evidence retained',
      );
    }
    if (created.has(output)) {
      await checkOwner(output);
      await writeFile(join(output, 'result.json'), JSON.stringify(report, null, 2));
    }
    console.log(
      JSON.stringify({ qualification: 'installed-release', ...report, evidence: output }),
    );
  }
  if (failure) throw failure;
}

async function selfTest() {
  if (process.platform === 'win32') {
    const nonce = randomBytes(16).toString('hex');
    const parent = await realpath(tmpdir());
    const fixture = join(parent, `prism-wide-shortcut-${nonce}`);
    await noLinks(parent);
    await mkdir(fixture);
    const fixtureOwner = join(fixture, marker);
    await writeFile(fixtureOwner, nonce, { flag: 'wx' });
    try {
      const working = join(fixture, '한글 folder with spaces 試験');
      await mkdir(working);
      const target = join(working, 'inert source.txt');
      await writeFile(target, `inert fixture ${nonce}`, { flag: 'wx' });
      const fixtureArgs = `--fixture="${nonce} 한글 試験"`;
      const link = join(fixture, 'fixture.lnk');
      const result = JSON.parse(
        powershell(
          `
      Add-Type -TypeDefinition @'
${wideShortcutReader}
'@
      $c=ConvertFrom-Json -InputObject $env:PRISM_QUAL_FIXTURE
      [PrismWideShortcutReader]::CreateFixture($c.link,$c.target,$c.working,$c.arguments)
      $before=[Convert]::ToBase64String([IO.File]::ReadAllBytes($c.link))
      $oversized=$false
      try { [PrismWideShortcutReader]::CreateFixture($c.link,$c.target,$c.working,('x' * 32768)) }
      catch { if(!$_.Exception.ToString().Contains('Invalid or oversized fixture field.')){throw}; $oversized=$true }
      $nul=$false
      try { [PrismWideShortcutReader]::CreateFixture($c.link,$c.target,$c.working,('x' + [char]0)) }
      catch { if(!$_.Exception.ToString().Contains('Invalid or oversized fixture field.')){throw}; $nul=$true }
      @{
        actual=[PrismWideShortcutReader]::Read($c.link)
        bufferCharacters=[PrismWideShortcutReader]::BufferCharacters
        oversizedRejected=$oversized
        nulRejected=$nul
        invalidWritesPreservedFile=($before -eq [Convert]::ToBase64String([IO.File]::ReadAllBytes($c.link)))
      } | ConvertTo-Json -Depth 4 -Compress
    `,
          { PRISM_QUAL_FIXTURE: JSON.stringify({ link, target, working, arguments: fixtureArgs }) },
        ),
      );
      assert.equal(pathKey(result.actual.Target), pathKey(target));
      assert.equal(pathKey(result.actual.WorkingDirectory), pathKey(working));
      assert.equal(result.actual.Arguments, fixtureArgs);
      assert.equal(result.bufferCharacters, 32768);
      assert.equal(result.oversizedRejected, true);
      assert.equal(result.nulRejected, true);
      assert.equal(result.invalidWritesPreservedFile, true);
      console.log(
        'Actual temp-only IShellLinkW Unicode target/working-directory/arguments roundtrip and buffer bounds passed.',
      );
    } finally {
      assert.equal(pathKey(await realpath(fixture)), pathKey(fixture));
      assert.equal(pathKey(dirname(fixture)), pathKey(parent));
      assert.equal(await readFile(fixtureOwner, 'utf8'), nonce);
      await rm(fixture, { recursive: true });
    }
    // Exercise the actual probe helper with in-memory substitutes. No registry or files.
    const definition = systemState.toString().match(/function Value\(\$path\) \{[^\n]+\}/)?.[0];
    assert(definition, 'Expected the actual PowerShell registry value helper');
    const values = JSON.parse(
      powershell(`
      function Test-Path { param($LiteralPath) $script:present }
      function Get-Item { param($LiteralPath) $script:item }
      class InertRegistryValue {
        [object] $Value
        [object] GetValue([string] $name, [object] $default) { return $this.Value }
      }
      $script:item=[InertRegistryValue]::new()
      ${definition}
      $script:present=$true
      $script:item.Value=[byte[]]@()
      $binary=Value 'inert-memory-fixture'
      $script:item.Value=''
      $text=Value 'inert-memory-fixture'
      $script:item.Value=$null
      $missingValue=Value 'inert-memory-fixture'
      $script:present=$false
      @{ emptyBinary=$binary; emptyString=$text; missingValue=$missingValue; absent=(Value 'inert-memory-fixture'); unicode='한글 試験' } | ConvertTo-Json -Compress
    `),
    );
    assert.deepEqual(values, {
      emptyBinary: [],
      emptyString: '',
      missingValue: null,
      absent: null,
      unicode: '한글 試験',
    });
  }
  const env = {
    GITHUB_ACTIONS: 'true',
    RUNNER_ENVIRONMENT: 'github-hosted',
    RUNNER_OS: 'Windows',
    RUNNER_ARCH: 'X64',
    PRISM_INSTALLED_RELEASE_QUALIFICATION: '1',
    GITHUB_RUN_ID: '123',
    RUNNER_TEMP: 'C:\\runner\\temp',
    GITHUB_WORKSPACE: 'D:\\a\\prism',
    APPDATA: 'C:\\profile\\Roaming',
    LOCALAPPDATA: 'C:\\profile\\Local',
  };
  requireHostedWindows('win32', 'x64', env);
  assert.throws(() =>
    requireHostedWindows(process.platform === 'win32' ? 'linux' : process.platform, 'x64', env),
  );
  assert.throws(() => requireHostedWindows('win32', 'arm64', env));
  for (const key of Object.keys(env))
    assert.throws(() => requireHostedWindows('win32', 'x64', { ...env, [key]: '' }));
  for (const extra of [
    { RUNNER_ENVIRONMENT: 'self-hosted' },
    { PRISM_DEV_DATA_DIR: '' },
    { PRISM_SETUP_SANDBOX: 'C:\\sandbox' },
    { PRISM_DEV_TEST_MODE: '1' },
  ])
    assert.throws(() => requireHostedWindows('win32', 'x64', { ...env, ...extra }));
  const nonce = 'a'.repeat(32);
  const root = 'C:\\profile\\app.prism.desktop';
  assertOwnedPath(root, [root], nonce);
  for (const path of [
    'C:\\',
    'C:\\profile',
    root + '-other',
    root + '\\child',
    root + '\\..\\app.prism.desktop',
    'relative',
  ])
    assert.throws(() => assertOwnedPath(path, [root], nonce));
  assert.throws(() => assertOwnedPath(root, [root], 'wrong'));
  const pristine = {
    registration: null,
    machine: false,
    startup: null,
    approved: null,
    machineStartup: null,
    machineApproved: null,
    processes: [],
  };
  verifyPristineSystem(pristine);
  for (const change of [
    { registration: {} },
    { machine: true },
    { startup: '' },
    { approved: [] },
    { machineStartup: '' },
    { machineApproved: [] },
    { processes: [{ ProcessId: 123 }] },
  ])
    assert.throws(() => verifyPristineSystem({ ...pristine, ...change }));
  const runtime = {
    native: true,
    platform: 'windows',
    development: false,
    installKind: 'installed',
    startupError: null,
  };
  const settings = {
    scopes: [
      {
        id: `installed_release_${nonce}`,
        path: root,
        filenameEnabled: false,
        contentEnabled: false,
        externalAllowed: false,
      },
    ],
    startOnLogin: false,
    clipboard: { enabled: false },
    updates: { autoCheck: false },
    ocr: { enabled: false },
    automaticSemantic: false,
    generation: { local: null, external: null, allowExternalQuestions: false, fallback: false },
    mcp: { enabled: false, servers: [] },
  };
  verifyIdentity(runtime, settings, nonce, root);
  for (const change of [
    { development: true },
    { installKind: 'portable' },
    { installKind: 'isolated' },
  ])
    assert.throws(() => verifyIdentity({ ...runtime, ...change }, settings, nonce, root));
  assert.throws(() => verifyIdentity(runtime, settings, 'b'.repeat(32), root));
  assert.throws(() => verifyIdentity(runtime, settings, nonce, root + '-foreign'));
  console.log(
    'Installed-release guard, exact ownership, and nonce identity checks passed (no installed app or registry mutation).',
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length === 3 && process.argv[2] === '--self-test') await selfTest();
  else {
    assert.equal(
      process.argv.length,
      2,
      'Only --self-test or the guarded CI qualification is supported',
    );
    await qualify();
  }
}
