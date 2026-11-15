import assert from 'node:assert/strict';

const phases = [
  'receiveFetch',
  'receiveCallback',
  'receiveQueue',
  'prepareRows',
  'insertEntries',
  'publish',
  'query',
  'queryScan',
];

// Completed scopes since process start. Thread-exclusive CPU is additive;
// inclusive CPU and wall time overlap and must not be added across phases.
export function summarizeFilenameCpu(profile, job) {
  assert.equal(profile?.schemaVersion, 1, 'The app must support filename CPU profiling');
  assert.equal(profile.sinceProcessStart, true);
  assert.equal(profile.completedScopesOnly, true);
  assert.equal(profile.lostMeasurements, 0, 'Counter failures invalidate CPU attribution');
  assert.deepEqual(profile.phases.map((row) => row.phase).sort(), [...phases].sort());
  const exclusivePhaseCpuMs = {};
  for (const row of profile.phases) {
    assert(Number.isSafeInteger(row.calls) && row.calls >= 0);
    for (const metric of ['wallNs', 'userCpu100ns', 'kernelCpu100ns', 'threadCycles']) {
      assert(Number.isSafeInteger(row.inclusive[metric]) && row.inclusive[metric] >= 0);
      assert(Number.isSafeInteger(row.exclusive[metric]) && row.exclusive[metric] >= 0);
      assert(row.exclusive[metric] <= row.inclusive[metric]);
    }
    exclusivePhaseCpuMs[row.phase] =
      (row.exclusive.userCpu100ns + row.exclusive.kernelCpu100ns) / 10_000;
  }
  const cpu = (row) => {
    assert(Number.isFinite(row.cpuUserMs) && row.cpuUserMs >= 0);
    assert(Number.isFinite(row.cpuKernelMs) && row.cpuKernelMs >= 0);
    return row.cpuUserMs + row.cpuKernelMs;
  };
  let prismCpuMs = 0;
  let liveWebViewCpuMs = 0;
  let liveOtherCpuMs = 0;
  for (const row of job.processes) {
    if (/^prism(?:\.exe)?$/i.test(row.name)) prismCpuMs += cpu(row);
    else if (/^msedgewebview2(?:\.exe)?$/i.test(row.name)) liveWebViewCpuMs += cpu(row);
    else liveOtherCpuMs += cpu(row);
  }
  const jobCpuMs = cpu(job);
  const exclusivePhaseTotalMs = Object.values(exclusivePhaseCpuMs).reduce((a, b) => a + b, 0);
  // Job helper milliseconds are rounded down; the Rust counters keep 100ns
  // ticks. Sampling follows diagnostics, so allow only rounding slack here.
  assert(exclusivePhaseTotalMs <= prismCpuMs + 10, 'Phase CPU cannot exceed sampled Prism CPU');
  return {
    exclusivePhaseCpuMs,
    exclusivePhaseTotalMs,
    prismCpuMs,
    prismCpuOutsideCompletedPhasesMs: Math.max(0, prismCpuMs - exclusivePhaseTotalMs),
    liveWebViewCpuMs,
    liveOtherCpuMs,
    exitedOrUnlistedCpuMs: Math.max(0, jobCpuMs - prismCpuMs - liveWebViewCpuMs - liveOtherCpuMs),
    jobCpuMs,
    completedScopesOnly: true,
    processBucketsCoverLiveProcessesOnly: true,
  };
}
