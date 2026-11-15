import assert from 'node:assert/strict';
import test from 'node:test';
import { summarizeFilenameCpu } from '../native/support/filename-cpu.mjs';
import { configureFilenameCpuTestEnvironment } from '../native/support/filename-cpu-env.mjs';

test('normal fixture launch removes inherited profiler activation', () => {
  const env = {
    PRISM_PROFILE_FILENAME_CPU: '1',
    PRISM_DEV_FILENAME_CPU_PROFILE: '1',
    PRISM_DEV_FILENAME_CPU_TEST_MODE: '1',
    prism_profile_filename_cpu: '1',
    prism_dev_filename_cpu_profile: '1',
    Prism_Dev_Filename_Cpu_Test_Mode: '1',
    PRISM_DEV_TEST_MODE: 'unchanged',
    WEBVIEW2_USER_DATA_FOLDER: 'owned-profile',
  };
  configureFilenameCpuTestEnvironment(env, false);
  assert.deepEqual(env, {
    PRISM_DEV_TEST_MODE: 'unchanged',
    WEBVIEW2_USER_DATA_FOLDER: 'owned-profile',
  });
});

test('explicit profiling restores only its dedicated flags after production sanitization', () => {
  const env = { WEBVIEW2_USER_DATA_FOLDER: 'owned-profile' };
  configureFilenameCpuTestEnvironment(env, true);
  assert.deepEqual(env, {
    WEBVIEW2_USER_DATA_FOLDER: 'owned-profile',
    PRISM_DEV_FILENAME_CPU_PROFILE: '1',
    PRISM_DEV_FILENAME_CPU_TEST_MODE: '1',
  });
  assert.equal(env.PRISM_DEV_TEST_MODE, undefined);
  assert.equal(env.PRISM_DEV_DATA_DIR, undefined);
});

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
