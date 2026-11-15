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

// Completed scopes since process start. Keep coarse thread-time ticks as raw
// diagnostic values, not exact short-phase seconds or a residual CPU estimate.
// Exclusive cycles are not converted to time; wall/inclusive values overlap.
export function summarizeFilenameCpu(profile, job) {
  assert.equal(profile?.schemaVersion, 2, 'The app must support filename cycle attribution');
  assert.equal(profile.sinceProcessStart, true);
  assert.equal(profile.completedScopesOnly, true);
  assert.equal(profile.lostMeasurements, 0, 'Counter failures invalidate CPU attribution');
  assert.equal(profile.threadCpuTicksAreCoarse, true);
  assert.equal(profile.cyclesConvertedToTime, false);
  assert(Number.isSafeInteger(profile.processCycles) && profile.processCycles >= 0);
  assert.deepEqual(profile.phases.map((row) => row.phase).sort(), [...phases].sort());
  const reportedThreadCpuTickMs = {};
  const exclusivePhaseCycles = {};
  for (const row of profile.phases) {
    assert(Number.isSafeInteger(row.calls) && row.calls >= 0);
    for (const metric of ['wallNs', 'userCpu100ns', 'kernelCpu100ns', 'threadCycles']) {
      assert(Number.isSafeInteger(row.inclusive[metric]) && row.inclusive[metric] >= 0);
      assert(Number.isSafeInteger(row.exclusive[metric]) && row.exclusive[metric] >= 0);
      assert(row.exclusive[metric] <= row.inclusive[metric]);
    }
    reportedThreadCpuTickMs[row.phase] =
      (row.exclusive.userCpu100ns + row.exclusive.kernelCpu100ns) / 10_000;
    exclusivePhaseCycles[row.phase] = row.exclusive.threadCycles;
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
  const completedPhaseCycles = Object.values(exclusivePhaseCycles).reduce((a, b) => a + b, 0);
  assert(completedPhaseCycles <= profile.processCycles);
  return {
    reportedThreadCpuTickMs,
    threadCpuTicksAreCoarse: true,
    exclusivePhaseCycles,
    completedPhaseCycles,
    fractionOfCompletedPhaseCycles: Object.fromEntries(
      Object.entries(exclusivePhaseCycles).map(([phase, cycles]) => [
        phase,
        completedPhaseCycles ? cycles / completedPhaseCycles : null,
      ]),
    ),
    processCycles: profile.processCycles,
    completedPhaseCycleCoverage: profile.processCycles
      ? completedPhaseCycles / profile.processCycles
      : null,
    processCyclesOutsideCompletedPhases: profile.processCycles - completedPhaseCycles,
    cyclesConvertedToTime: false,
    prismCpuMs,
    liveWebViewCpuMs,
    liveOtherCpuMs,
    exitedOrUnlistedCpuMs: Math.max(0, jobCpuMs - prismCpuMs - liveWebViewCpuMs - liveOtherCpuMs),
    jobCpuMs,
    completedScopesOnly: true,
    processBucketsCoverLiveProcessesOnly: true,
  };
}
