// Inert stream scheduling only: no network, app, official transfer or model.
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { test } from 'node:test';
import { resumeWhenReady } from '../native/support/throttle-proxy.mjs';

test('a drain before the throttle delay cannot strand a paused tunnel', async (t) => {
  const upstream = new PassThrough();
  upstream.pause();
  const client = new Writable({
    highWaterMark: 1,
    write(_chunk, _encoding, done) {
      queueMicrotask(done);
    },
  });
  t.after(() => {
    client.destroy();
    upstream.destroy();
  });
  const drained = client.write(Buffer.from('inert bytes'));
  assert.equal(drained, false);
  const ready = once(upstream, 'resume', { signal: AbortSignal.timeout(1000) });
  const drain = once(client, 'drain');
  resumeWhenReady(client, upstream, 30, drained);
  await drain;
  assert.equal(upstream.isPaused(), true, 'drain alone must not bypass the rate delay');
  await ready;
  assert.equal(upstream.isPaused(), false);
});

test('a completed rate delay still waits for the destination to drain', async (t) => {
  let release;
  const client = new Writable({
    highWaterMark: 1,
    write(_chunk, _encoding, done) {
      release = done;
    },
  });
  const upstream = new PassThrough();
  upstream.pause();
  t.after(() => {
    client.destroy();
    upstream.destroy();
  });
  assert.equal(client.write(Buffer.from('inert bytes')), false);
  const ready = once(upstream, 'resume', { signal: AbortSignal.timeout(1000) });
  resumeWhenReady(client, upstream, 5, false);
  await new Promise((done) => setTimeout(done, 20));
  assert.equal(upstream.isPaused(), true);
  release();
  await ready;
  assert.equal(upstream.isPaused(), false);
});

for (const side of ['client', 'upstream']) {
  test(`closing the ${side} removes the pending drain and rate wait`, async (t) => {
    const client = new PassThrough();
    const upstream = new PassThrough();
    upstream.pause();
    t.after(() => {
      client.destroy();
      upstream.destroy();
    });
    let resumed = false;
    upstream.on('resume', () => {
      resumed = true;
    });
    resumeWhenReady(client, upstream, 10_000, false);
    const closing = side === 'client' ? client : upstream;
    const closed = once(closing, 'close');
    closing.destroy();
    await closed;
    assert.equal(resumed, false);
    assert.equal(client.listenerCount('drain'), 0);
    assert.equal(upstream.listenerCount('close'), 0);
  });
}
