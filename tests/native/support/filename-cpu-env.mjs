// A caller must explicitly request profiling in this owned native fixture.
// Never inherit profiler activation from the user's launch environment.
export function configureFilenameCpuTestEnvironment(environment, enabled) {
  const names = new Set([
    'PRISM_PROFILE_FILENAME_CPU',
    'PRISM_DEV_FILENAME_CPU_PROFILE',
    'PRISM_DEV_FILENAME_CPU_TEST_MODE',
  ]);
  for (const name of Object.keys(environment)) {
    if (names.has(name.toUpperCase())) delete environment[name];
  }
  if (enabled === true) {
    environment.PRISM_DEV_FILENAME_CPU_PROFILE = '1';
    environment.PRISM_DEV_FILENAME_CPU_TEST_MODE = '1';
  }
}
