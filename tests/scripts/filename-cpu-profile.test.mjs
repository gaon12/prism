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
      schemaVersion: 1,
      sinceProcessStart: true,
      completedScopesOnly: true,
      lostMeasurements: 0,
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

test('adds exclusive CPU only and keeps unlisted/exited process CPU visible', () => {
  const { profile, job } = fixture();
  const result = summarizeFilenameCpu(profile, job);
  assert.equal(result.exclusivePhaseTotalMs, 8);
  assert.equal(result.prismCpuOutsideCompletedPhasesMs, 4);
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
