import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { bundleName, stampMetainfo, TRAY_LIBRARIES } from '../../scripts/build-flatpak.mjs';

const manifest = readFileSync(
  new URL('../../installer/flatpak/app.prism.desktop.yml', import.meta.url),
  'utf8',
);
const metainfo = readFileSync(
  new URL('../../installer/flatpak/app.prism.desktop.metainfo.xml', import.meta.url),
  'utf8',
);

test('bundle names match the AppImage architectures and reject others', () => {
  assert.equal(bundleName('0.1.0', 'amd64'), 'Prism_0.1.0_amd64.flatpak');
  assert.equal(bundleName('0.1.0', 'aarch64'), 'Prism_0.1.0_aarch64.flatpak');
  assert.throws(() => bundleName('0.1.0', 'x86_64'), /Unsupported/);
});

test('the metainfo release follows the build version', () => {
  const stamped = stampMetainfo(metainfo, '1.2.3', '2026-12-01');
  assert.match(stamped, /<release version="1.2.3" date="2026-12-01" \/>/);
  assert.equal(stamped.match(/<release /g).length, 1);
  assert.throws(() => stampMetainfo(metainfo, '1.2', '2026-12-01'), /Invalid version/);
  assert.throws(() => stampMetainfo('<component/>', '1.2.3', '2026-12-01'), /no release/);
});

test('the manifest keeps the agreed sandbox and the app id everywhere', () => {
  assert.match(manifest, /^id: app\.prism\.desktop$/m);
  assert.match(manifest, /^command: prism$/m);
  // User decision 2026-11-12: whole-host file access for indexing.
  assert.match(manifest, /- --filesystem=host$/m);
  for (const permission of ['--share=network', '--socket=wayland', '--socket=fallback-x11'])
    assert.ok(manifest.includes(`- ${permission}`), permission);
  assert.match(metainfo, /<id>app\.prism\.desktop<\/id>/);
  assert.match(
    metainfo,
    /<launchable type="desktop-id">app\.prism\.desktop\.desktop<\/launchable>/,
  );
  // The tray library the binary loads at runtime is staged with its dependencies.
  assert.deepEqual(TRAY_LIBRARIES, [
    'libappindicator3.so.1',
    'libdbusmenu-glib.so.4',
    'libdbusmenu-gtk3.so.4',
  ]);
  assert.ok(manifest.includes('install -Dm644 -t /app/lib tray/*'));
});
