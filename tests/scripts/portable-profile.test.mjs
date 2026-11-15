import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { lstat, mkdir, mkdtemp, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createServer, Server } from 'node:net';
import { syncBuiltinESMExports } from 'node:module';
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

async function occupiedPort(t) {
  let connections = 0;
  const server = createServer((socket) => {
    connections++;
    socket.destroy();
  });
  await new Promise((done, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', done);
  });
  const close = async () => {
    if (server.listening) await new Promise((done) => server.close(done));
  };
  t.after(close);
  return { port: server.address().port, server, close, connections: () => connections };
}

function observeCollisions(t, port) {
  let first,
    second,
    count = 0;
  const firstCollision = new Promise((done) => {
    first = done;
  });
  const secondCollision = new Promise((done) => {
    second = done;
  });
  const listen = Server.prototype.listen;
  // Observe real socket errors without replacing bind or releasing the listener.
  t.mock.method(Server.prototype, 'listen', function (...args) {
    if (args[0] === port && args[1] === '127.0.0.1') {
      this.once('error', (error) => {
        if (error.code !== 'EADDRINUSE') return;
        count++;
        if (count === 1) first();
        if (count === 2) second();
      });
    }
    return Reflect.apply(listen, this, args);
  });
  return { firstCollision, secondCollision, count: () => count };
}

function stopBeforeSpawn(t, callback) {
  // Sync the harness's named builtin import so interception precedes the OS.
  const stub = t.mock.method(childProcess, 'spawn', callback);
  syncBuiltinESMExports();
  t.after(() => {
    stub.mock.restore();
    syncBuiltinESMExports();
  });
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

test(
  'an explicit CDP port waits for release before reaching spawn',
  { timeout: 5000 },
  async (t) => {
    const { executable, profile } = await fixture(t);
    await writeFile(join(dirname(executable), 'prism.portable'), '');
    const occupied = await occupiedPort(t);
    const observed = observeCollisions(t, occupied.port);
    let released = false;
    let spawned = false;
    const stop = new Error('Stop at the spawn boundary; never launch a real app');
    stopBeforeSpawn(t, () => {
      spawned = true;
      assert.equal(released, true, 'The existing listener must close before spawn');
      throw stop;
    });
    const outcome = launch(profile, {
      executable,
      portable: true,
      productionPortable: true,
      cdpPort: occupied.port,
    }).then(
      () => new Error('Unexpected UI launch'),
      (error) => error,
    );
    assert.equal(
      await Promise.race([observed.firstCollision.then(() => true), outcome.then(() => false)]),
      true,
      'The real fixed-port reservation must encounter the owned listener',
    );
    assert.equal(spawned, false);
    assert.equal(occupied.server.listening, true);
    assert.equal(occupied.connections(), 0, 'Never connect to the existing endpoint');
    await occupied.close();
    released = true;
    assert.equal(await outcome, stop, 'A transient occupied port must be retried after release');
    assert.equal(spawned, true);
    assert.equal(occupied.connections(), 0);
  },
);

test(
  'a CDP port that stays occupied expires without spawning or connecting',
  { timeout: 5000 },
  async (t) => {
    const { executable, profile } = await fixture(t);
    await writeFile(join(dirname(executable), 'prism.portable'), '');
    const occupied = await occupiedPort(t);
    const observed = observeCollisions(t, occupied.port);
    let now = 100_000;
    // Only the guard's wall clock advances; real bind failures and retry timers run.
    t.mock.method(Date, 'now', () => now);
    let spawned = false;
    stopBeforeSpawn(t, () => {
      spawned = true;
      throw new Error('An occupied port must not reach spawn');
    });
    const outcome = launch(profile, {
      executable,
      portable: true,
      productionPortable: true,
      cdpPort: occupied.port,
    }).then(
      () => new Error('Unexpected UI launch'),
      (error) => error,
    );
    assert.equal(
      await Promise.race([observed.secondCollision.then(() => true), outcome.then(() => false)]),
      true,
      'An explicit EADDRINUSE must be retried before its deadline',
    );
    now += 20_001;
    assert.equal((await outcome).code, 'EADDRINUSE');
    assert(observed.count() >= 2);
    assert.equal(occupied.server.listening, true);
    assert.equal(occupied.connections(), 0);
    assert.equal(spawned, false);
  },
);
