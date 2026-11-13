import assert from 'node:assert/strict';
import { lstat, mkdir, mkdtemp, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative } from 'node:path';
import { test } from 'node:test';
import { assertPortableProfile, launch } from '../native/harness.mjs';

async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'prism-native-profile-'));
  t.after(async () => {
    const withinTemp = relative(await realpath(tmpdir()), await realpath(directory));
    assert(withinTemp && !withinTemp.startsWith('..') && !isAbsolute(withinTemp));
    assert(basename(directory).startsWith('prism-native-profile-'));
    await rm(directory, { recursive: true });
  });
  const executable = join(directory, 'app', 'prism.exe');
  await mkdir(dirname(executable));
  // Invalid PE and non-executable mode: the negative control can never launch
  // a real app or read/write an installed profile, even before the guard exists.
  await writeFile(executable, 'Not an executable: private preflight fixture', { mode: 0o600 });
  return { directory, executable, profile: join(dirname(executable), 'PrismData') };
}

test('a missing portable marker is rejected before creating a child', async (t) => {
  const { executable, profile } = await fixture(t);
  let spawned = false;
  let failure;
  try {
    await launch(profile, {
      executable,
      portable: true,
      productionPortable: true,
      onSpawn: () => {
        spawned = true;
      },
    });
  } catch (error) {
    failure = error;
  }
  assert.equal(spawned, false, 'Unsafe portable input must never reach spawn');
  assert.match(String(failure), /portable marker/i);
});

test('a valid portable fixture is checked without creating its profile', async (t) => {
  const { executable, profile } = await fixture(t);
  await writeFile(join(dirname(executable), 'prism.portable'), '');
  await assertPortableProfile(profile, executable);
  await assert.rejects(lstat(profile), { code: 'ENOENT' });
  await mkdir(profile);
  await assertPortableProfile(profile, executable);
});

test('a different requested profile is rejected before creating a child', async (t) => {
  const { directory, executable } = await fixture(t);
  await writeFile(join(dirname(executable), 'prism.portable'), '');
  let spawned = false;
  await assert.rejects(
    launch(join(directory, 'different-profile'), {
      executable,
      portable: true,
      onSpawn: () => {
        spawned = true;
      },
    }),
    /data directory must be PrismData/,
  );
  assert.equal(spawned, false);
});

test('a directory cannot stand in for the portable marker', async (t) => {
  const { executable, profile } = await fixture(t);
  await mkdir(join(dirname(executable), 'prism.portable'));
  await assert.rejects(assertPortableProfile(profile, executable), /portable marker/i);
});

test('a file cannot stand in for the data directory', async (t) => {
  const { executable, profile } = await fixture(t);
  await writeFile(join(dirname(executable), 'prism.portable'), '');
  await writeFile(profile, 'not a directory');
  await assert.rejects(assertPortableProfile(profile, executable), /data directory must not/);
});

test('a redirected data directory cannot point the fixture elsewhere', async (t) => {
  const { directory, executable, profile } = await fixture(t);
  await writeFile(join(dirname(executable), 'prism.portable'), '');
  const target = join(directory, 'other-owned-directory');
  await mkdir(target);
  await symlink(target, profile, process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(assertPortableProfile(profile, executable), /data directory must not/);
});
