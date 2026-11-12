// Low-end machine filename-index measurement (Windows).
//
// The product baseline is a weak PC: 1 core / 1 thread, 512 MB RAM and an HDD.
// The development PC is much faster, so this check confines the owned app (and
// every WebView/child process it starts) to that CPU and memory budget with a
// real Windows Job Object (support/job-limit.ps1), then measures how long the
// whole-drive filename list takes to become ready.
//
// What it measures: app launch to a completed first filename sweep of the
// selected drive, the app's own service-snapshot timing breakdown, and peak
// committed memory. Run it twice in a row to see a "cached start": the
// installed NTFS service keeps its USN cache between requests.
//
// What it does NOT confine: the installed NTFS service runs as LocalSystem and
// needs administrator rights to join a job; HDD latency is not emulated here.
// Both are recorded as explicit limitations in result.json.
//
//   $env:PRISM_TEST_EXECUTABLE = 'C:\path\to\portable\prism.exe'
//   node tests/native/harsh-profile.mjs
//
// Optional: PRISM_HARSH_SCOPE (default C:\), PRISM_HARSH_CPUS (1),
// PRISM_HARSH_MEMORY_MB (512), PRISM_HARSH_TIMEOUT_MS (600000).
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { launch, shutdown, until } from './harness.mjs';

/**
 * Add each WebView process's role (`--type=renderer`, `gpu-process`, ...;
 * none for the browser process) from its command line, so memory growth can
 * be attributed. Read-only; a failed lookup just leaves the roles out.
 */
function labelProcesses(processes) {
  if (!processes?.length) return;
  try {
    const ids = processes.map((row) => Number(row.pid)).filter(Number.isInteger);
    const script = `Get-CimInstance Win32_Process -Filter "${ids.map((id) => `ProcessId=${id}`).join(' OR ')}" | ForEach-Object { "$($_.ProcessId) $($_.CommandLine)" }`;
    const lines = execFileSync('powershell', ['-NoProfile', '-Command', script], {
      encoding: 'utf8',
      windowsHide: true,
    }).split(/\r?\n/);
    for (const row of processes) {
      const line = lines.find((entry) => entry.startsWith(`${row.pid} `)) ?? '';
      row.role = /--type=([\w-]+)/.exec(line)?.[1] ?? (line ? 'main' : 'unknown');
    }
  } catch {
    // Diagnostics only.
  }
}

assert.equal(process.platform, 'win32', 'The Job Object profile is Windows-only');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Set PRISM_TEST_EXECUTABLE to a release prism.exe');
const candidate = resolve(process.env.PRISM_TEST_EXECUTABLE);
const scopePath = resolve(process.env.PRISM_HARSH_SCOPE ?? 'C:\\');
const cpus = Number(process.env.PRISM_HARSH_CPUS ?? 1);
const memoryMB = Number(process.env.PRISM_HARSH_MEMORY_MB ?? 512);
const timeoutMs = Number(process.env.PRISM_HARSH_TIMEOUT_MS ?? 600_000);
const helperPath = fileURLToPath(new URL('./support/job-limit.ps1', import.meta.url));

// Own a fresh portable copy: the user's profile and the candidate stay untouched.
const directory = await mkdtemp(join(tmpdir(), 'prism-harsh-profile-'));
const application = join(directory, 'app');
const profile = join(application, 'PrismData');
const evidence = join(directory, 'evidence');
await Promise.all([profile, evidence].map((path) => mkdir(path, { recursive: true })));
const executable = join(application, 'prism.exe');
await copyFile(candidate, executable);
await writeFile(join(application, 'prism.portable'), '');
const scopeId = `harsh_${Date.now()}`;
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    startOnLogin: false,
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    ocr: { enabled: false },
    automaticSemantic: false,
    hybridSearch: false,
    // Filename-only: no document content is read during this measurement.
    scopes: [
      { id: scopeId, name: 'Drive', path: scopePath, filenameEnabled: true, contentEnabled: false },
    ],
  }),
);

/**
 * Start job-limit.ps1 before the app exists: it compiles and creates the job
 * with its limits first, so assigning the spawned app takes only milliseconds
 * and the WebView processes the app starts later join the same job.
 */
function startJob() {
  const helper = spawn(
    'pwsh',
    ['-NoProfile', '-File', helperPath, '-Cpus', String(cpus), '-MemoryMB', String(memoryMB)],
    { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true },
  );
  const lines = createInterface({ input: helper.stdout });
  const pending = [];
  lines.on('line', (line) => pending.shift()?.(JSON.parse(line)));
  let stderr = '';
  helper.stderr.on('data', (chunk) => (stderr += chunk));
  const next = () =>
    new Promise((done, fail) => {
      pending.push(done);
      helper.once('exit', () => fail(new Error(`Job helper exited: ${stderr.trim()}`)));
    });
  return {
    prepared: next(),
    /** Assign the app; resolves with the limits read back from the kernel. */
    assign: (pid) => {
      const answer = next();
      helper.stdin.write(`assign ${pid}\n`);
      return answer;
    },
    report: () => {
      const answer = next();
      helper.stdin.write('report\n');
      return answer;
    },
    close: () => helper.stdin.end('exit\n'),
  };
}

const result = {
  passed: false,
  profile: { cpus, memoryMB, scope: scopePath },
  limitations: [
    'The installed NTFS service process is not confined (LocalSystem needs administrator rights).',
    'The job limit covers the app and the processes it starts after assignment (WebView); see job.processIds.',
    'HDD seek/throughput is not emulated; the drive is the development NVMe SSD.',
  ],
};
let instance;
let job;
let assigned;
let spawnedAt;
try {
  job = startJob();
  await job.prepared;
  instance = await launch(profile, {
    executable,
    portable: true,
    productionPortable: true,
    onSpawn: (owned) => {
      spawnedAt = performance.now();
      assigned = job.assign(owned.pid);
    },
  });
  result.limits = await assigned;
  result.assignMs = Math.round(performance.now() - spawnedAt);
  const { invoke } = instance;
  const settings = await invoke('get_settings');
  assert(settings.scopes[0]?.id === scopeId, 'CDP must reach this fixture profile');
  result.runtime = await invoke('runtime_info');
  assert.equal(result.runtime.installKind, 'portable');
  result.service = await invoke('index_service_status').catch(() => null);
  let status;
  await until(
    async () => {
      status = await invoke('index_status');
      // The first moment a search can find files: a first scan's rows are
      // searchable before the whole snapshot is validated.
      if (result.firstSearchableMs === undefined && status.count > 0) {
        const partial = await invoke('search_files', { query: '', extension: '', scopeId });
        if (partial.total > 0) {
          result.firstSearchableMs = Math.round(performance.now() - spawnedAt);
          result.firstSearchableTotal = partial.total;
        }
      }
      return status.initialScanComplete && !status.scanning;
    },
    'first complete filename sweep',
    timeoutMs,
  );
  result.readyMs = Math.round(performance.now() - spawnedAt);
  result.count = status.count;
  result.skipped = status.skipped;
  const diagnostics = await invoke('diagnostics_snapshot');
  result.lastServiceSnapshot = diagnostics?.filenameIndex?.lastServiceSnapshot ?? null;
  // A search over the full catalog must also stay usable inside the budget.
  const searchStart = performance.now();
  const search = await invoke('search_files', { query: 'readme', extension: '', scopeId });
  result.searchMs = Math.round(performance.now() - searchStart);
  result.searchTotal = search.total;
  // Renderer-side numbers for each window, to tell how much of a WebView
  // renderer's memory is the app's own JavaScript heap and DOM.
  result.pages = {};
  for (const [name, page] of [
    ['main', instance.main],
    ['quick', instance.quick],
  ]) {
    const session = await page.context().newCDPSession(page);
    await session.send('Performance.enable');
    const { metrics } = await session.send('Performance.getMetrics');
    await session.detach();
    const value = (key) => metrics.find((metric) => metric.name === key)?.value ?? null;
    result.pages[name] = {
      jsHeapUsedBytes: value('JSHeapUsedSize'),
      jsHeapTotalBytes: value('JSHeapTotalSize'),
      domNodes: value('Nodes'),
      documents: value('Documents'),
      jsEventListeners: value('JSEventListeners'),
    };
  }
  result.job = await job.report();
  labelProcesses(result.job.processes);
  result.passed = true;
} catch (error) {
  result.error = String(error?.stack ?? error);
  if (job) result.job = await job.report().catch(() => null);
} finally {
  if (instance) await shutdown(instance).catch(() => {});
  job?.close();
  await writeFile(join(evidence, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify({ evidence, ...result }, null, 2));
}
if (!result.passed) process.exitCode = 1;
