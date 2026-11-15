import assert from 'node:assert/strict';
import test from 'node:test';
import { summarizeFilenameCpu } from '../native/support/filename-cpu.mjs';

function fixture() {
  const cost = (cpu) => ({
    wallNs: 100_000,
    userCpu100ns: cpu,
    kernelCpu100ns: 0,
    threadCycles: 1_000,
  });
  return {
    profile: {
      schemaVersion: 2,
      sinceProcessStart: true,
      completedScopesOnly: true,
      lostMeasurements: 0,
      threadCpuTicksAreCoarse: true,
      cyclesConvertedToTime: false,
      processCycles: 10_000,
      phases: [
        'receiveFetch',
        'receiveCallback',
        'receiveQueue',
        'prepareRows',
        'insertEntries',
        'publish',
        'query',
        'queryScan',
      ].map((phase) => ({ phase, calls: 1, inclusive: cost(30_000), exclusive: cost(10_000) })),
    },
    job: {
      cpuUserMs: 35,
      cpuKernelMs: 5,
      processes: [
        { name: 'prism', cpuUserMs: 10, cpuKernelMs: 2 },
        { name: 'msedgewebview2', cpuUserMs: 20, cpuKernelMs: 2 },
        { name: 'other', cpuUserMs: 1, cpuKernelMs: 1 },
      ],
    },
  };
}

test('adds exclusive cycles without converting them to time and keeps unlisted CPU visible', () => {
  const { profile, job } = fixture();
  const result = summarizeFilenameCpu(profile, job);
  assert.equal(result.completedPhaseCycles, 8_000);
  assert.equal(result.processCyclesOutsideCompletedPhases, 2_000);
  assert.equal(result.completedPhaseCycleCoverage, 0.8);
  assert.equal(result.fractionOfCompletedPhaseCycles.query, 0.125);
  assert.equal(result.cyclesConvertedToTime, false);
  assert.equal(result.threadCpuTicksAreCoarse, true);
  assert.equal(result.prismCpuMs, 12);
  assert.equal(result.prismCpuOutsideCompletedPhasesMs, undefined);
  assert.equal(result.liveWebViewCpuMs, 22);
  assert.equal(result.liveOtherCpuMs, 2);
  assert.equal(result.exitedOrUnlistedCpuMs, 4);
});

test('rejects unavailable or incomplete counter evidence', () => {
  for (const mutate of [
    (p) => {
      p.lostMeasurements = 1;
    },
    (p) => {
      p.processCycles = 7_999;
    },
    (p) => {
      p.phases.pop();
    },
    (p) => {
      p.phases[0].exclusive.userCpu100ns = -1;
    },
    (p) => {
      p.phases[0].exclusive.threadCycles = 2_000;
    },
  ]) {
    const { profile, job } = fixture();
    mutate(profile);
    assert.throws(() => summarizeFilenameCpu(profile, job));
  }
});
