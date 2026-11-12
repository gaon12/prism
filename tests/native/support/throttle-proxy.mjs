// A loopback HTTPS CONNECT proxy that only slows the real transfer down.
//
// Why: hosted CI networks download the official model archives in a couple of
// seconds, so a native test cannot observe or cancel a *real* download in
// progress. This proxy keeps the transfer real: the app opens an ordinary TLS
// tunnel through it, verifies the official server certificate itself and
// hashes the bytes itself. The proxy never sees plaintext, never answers on the
// server's behalf and never changes a byte. It only delays relaying
// server-to-app bytes so the total rate stays near `bytesPerSecond`.
//
// Only `CONNECT host:443` is accepted; anything else is refused, so the proxy
// cannot become a general relay for other ports or plain-HTTP requests.
import net from 'node:net';

const MAX_HEADER_BYTES = 8 * 1024;

export function resumeWhenReady(client, upstream, wait, drained) {
  if (client.destroyed || upstream.destroyed) return;
  let timer;
  let delayDone = wait <= 0;
  let writable = drained;
  const cleanup = () => {
    clearTimeout(timer);
    client.off('drain', onDrain);
    client.off('close', cleanup);
    upstream.off('close', cleanup);
  };
  const resume = () => {
    if (!delayDone || !writable) return;
    cleanup();
    if (!client.destroyed && !upstream.destroyed) upstream.resume();
  };
  const onDrain = () => {
    writable = true;
    resume();
  };
  // Drain may happen while the rate timer is still pending. Observe both
  // conditions immediately, then resume once both are true in either order.
  client.once('close', cleanup);
  upstream.once('close', cleanup);
  if (!drained) client.once('drain', onDrain);
  if (wait > 0)
    timer = setTimeout(() => {
      delayDone = true;
      resume();
    }, wait);
  resume();
}

/**
 * Start the proxy on 127.0.0.1 with an OS-assigned port.
 * @param {number} bytesPerSecond Total server-to-app relay rate for all tunnels.
 * @returns {Promise<{url: string, stats: () => object, close: () => Promise<void>}>}
 */
export async function startThrottleProxy(bytesPerSecond) {
  if (!Number.isSafeInteger(bytesPerSecond) || bytesPerSecond < 64 * 1024) {
    throw new Error('Throttle rate must be an integer of at least 64 KiB/s');
  }
  const sockets = new Set();
  const hosts = new Set();
  let relayedBytes = 0;
  let refused = 0;
  // One schedule shared by every tunnel: `scheduled` bytes may have been sent
  // by `windowStart + scheduled / rate`. Writing ahead of that time pauses the
  // upstream socket until the schedule catches up.
  let windowStart = Date.now();
  let scheduled = 0;
  const delayAfter = (bytes) => {
    const now = Date.now();
    // After an idle gap, start a fresh window instead of allowing a burst.
    if (windowStart + (scheduled / bytesPerSecond) * 1000 < now) {
      windowStart = now;
      scheduled = 0;
    }
    scheduled += bytes;
    return Math.max(0, windowStart + (scheduled / bytesPerSecond) * 1000 - now);
  };

  const track = (socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
    // Errors end only that tunnel; the app sees a normal connection failure.
    socket.on('error', () => socket.destroy());
  };

  const server = net.createServer((client) => {
    track(client);
    let header = Buffer.alloc(0);
    const onHeader = (chunk) => {
      header = Buffer.concat([header, chunk]);
      const end = header.indexOf('\r\n\r\n');
      if (end < 0) {
        if (header.length > MAX_HEADER_BYTES) client.destroy();
        return;
      }
      client.off('data', onHeader);
      client.pause();
      const requestLine = header.subarray(0, header.indexOf('\r\n')).toString('latin1');
      const match = /^CONNECT ([A-Za-z0-9.-]{1,253}):443 HTTP\/1\.[01]$/.exec(requestLine);
      if (!match) {
        refused += 1;
        client.end('HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n');
        return;
      }
      const host = match[1];
      const upstream = net.connect({ host, port: 443 });
      track(upstream);
      upstream.once('connect', () => {
        hosts.add(host);
        client.write('HTTP/1.1 200 Connection Established\r\n\r\n');
        // Bytes the client already sent after the header (TLS ClientHello).
        const early = header.subarray(end + 4);
        if (early.length) upstream.write(early);
        // App-to-server traffic (requests, TLS handshakes) is not delayed.
        client.pipe(upstream);
        client.resume();
      });
      upstream.on('data', (chunk) => {
        relayedBytes += chunk.length;
        const drained = client.write(chunk);
        const wait = delayAfter(chunk.length);
        if (wait === 0 && drained) return;
        upstream.pause();
        resumeWhenReady(client, upstream, wait, drained);
      });
      upstream.on('end', () => client.end());
      client.on('end', () => upstream.end());
      upstream.on('close', () => client.destroy());
      client.on('close', () => upstream.destroy());
    };
    client.on('data', onHeader);
  });

  await new Promise((done, fail) => {
    server.once('error', fail);
    server.listen(0, '127.0.0.1', done);
  });
  const { port } = server.address();
  return {
    url: `http://127.0.0.1:${port}`,
    // Host names and byte counts only: no paths, headers or payload.
    stats: () => ({
      bytesPerSecond,
      relayedBytes,
      tunnelHosts: [...hosts].sort(),
      refusedRequests: refused,
    }),
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((done) => server.close(() => done()));
    },
  };
}
