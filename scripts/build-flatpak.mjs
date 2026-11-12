#!/usr/bin/env node
// Build the Prism Flatpak bundle from the .deb that `tauri build` produced, then
// install it and check it inside the sandbox.
//
//   node scripts/build-flatpak.mjs --deb <file.deb> --version 0.1.0 --arch amd64 --out out
//
// Needs `flatpak`, `flatpak-builder` and the Flathub remote (the workflow sets
// them up). The GNOME runtime/SDK named in the manifest are installed from
// Flathub when missing. Nothing is published: the result is a local bundle.
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const APP_ID = 'app.prism.desktop';
/** The tray library Tauri loads at runtime, with its dependencies. */
export const TRAY_LIBRARIES = [
  'libappindicator3.so.1',
  'libdbusmenu-glib.so.4',
  'libdbusmenu-gtk3.so.4',
];
const ARCHES = new Set(['amd64', 'aarch64']);

/** Set the AppStream release to this build's version (and today's date). */
export function stampMetainfo(text, version, date) {
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version))
    throw new Error(`Invalid version: ${version}`);
  const release = /<release version="[^"]*" date="[^"]*" \/>/;
  if (!release.test(text)) throw new Error('The metainfo has no release entry.');
  return text.replace(release, `<release version="${version}" date="${date}" />`);
}

/** The bundle's file name, matching the AppImage naming. */
export function bundleName(version, arch) {
  if (!ARCHES.has(arch)) throw new Error(`Unsupported Flatpak architecture: ${arch}`);
  return `Prism_${version}_${arch}.flatpak`;
}

/** Find a shared library on this build host by its soname (`ldconfig -p`). */
function hostLibrary(soname) {
  const listing = run('ldconfig', ['-p'], { capture: true });
  const line = listing.split('\n').find((entry) => entry.trim().startsWith(`${soname} `));
  const path = line?.split('=>').pop()?.trim();
  if (!path || !existsSync(path)) throw new Error(`Build host is missing ${soname}.`);
  return realpathSync(path);
}

function run(command, args, { capture = false, cwd } = {}) {
  const result = spawnSync(command, args, {
    cwd,
    encoding: 'utf8',
    stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(' ')} exited with ${result.status}`);
  return capture ? result.stdout : '';
}

function parse(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const name = argv[index];
    const value = argv[index + 1];
    if (!['--deb', '--version', '--arch', '--out'].includes(name) || value === undefined)
      throw new Error(
        `Usage: build-flatpak.mjs --deb <file> --version <v> --arch amd64|aarch64 --out <dir>`,
      );
    options[name.slice(2)] = value;
  }
  for (const name of ['deb', 'version', 'arch', 'out'])
    if (!options[name]) throw new Error(`Missing --${name}`);
  return options;
}

export function main(argv) {
  const { deb, version, arch, out } = parse(argv);
  const bundle = resolve(out, bundleName(version, arch));
  const sources = resolve(fileURLToPath(new URL('../installer/flatpak', import.meta.url)));
  const stage = mkdtempSync(join(process.env.RUNNER_TEMP || tmpdir(), 'prism-flatpak-'));
  try {
    // Stage the manifest and its inputs together: flatpak-builder resolves
    // source paths relative to the manifest.
    copyFileSync(join(sources, `${APP_ID}.yml`), join(stage, `${APP_ID}.yml`));
    const today = new Date().toISOString().slice(0, 10);
    const metainfo = readFileSync(join(sources, `${APP_ID}.metainfo.xml`), 'utf8');
    writeFileSync(join(stage, `${APP_ID}.metainfo.xml`), stampMetainfo(metainfo, version, today));
    copyFileSync(resolve(deb), join(stage, 'prism.deb'));
    mkdirSync(join(stage, 'tray'));
    for (const soname of TRAY_LIBRARIES)
      copyFileSync(hostLibrary(soname), join(stage, 'tray', soname));

    run('flatpak-builder', [
      '--user',
      '--install-deps-from=flathub',
      '--force-clean',
      '--disable-rofiles-fuse',
      `--repo=${join(stage, 'repo')}`,
      join(stage, 'build'),
      join(stage, `${APP_ID}.yml`),
    ]);
    mkdirSync(resolve(out), { recursive: true });
    run('flatpak', [
      'build-bundle',
      '--runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo',
      join(stage, 'repo'),
      bundle,
      APP_ID,
    ]);

    // Install the bundle and check it from inside the sandbox: every library
    // resolves against the runtime, and Tauri finds its resources.
    run('flatpak', ['install', '--user', '--noninteractive', '--reinstall', '-y', bundle]);
    const check = [
      'set -eu',
      'test -x /app/bin/prism',
      'test -s /app/lib/Prism/notices/pdf-renderer.txt',
      'test -s /app/share/applications/app.prism.desktop.desktop',
      'test -s /app/share/metainfo/app.prism.desktop.metainfo.xml',
      ...TRAY_LIBRARIES.map((name) => `test -s /app/lib/${name}`),
      'missing=$(ldd /app/bin/prism /app/lib/libappindicator3.so.1 | grep "not found" || true)',
      'if [ -n "$missing" ]; then echo "$missing"; exit 1; fi',
      'echo flatpak-sandbox-ok',
    ].join('\n');
    const output = run('flatpak', ['run', '--command=sh', APP_ID, '-c', check], { capture: true });
    if (!output.includes('flatpak-sandbox-ok'))
      throw new Error('The sandbox check did not finish.');
    console.log(JSON.stringify({ bundle: basename(bundle), sandboxCheck: 'passed' }));
  } finally {
    spawnSync('flatpak', ['uninstall', '--user', '--noninteractive', '-y', APP_ID], {
      stdio: 'ignore',
    });
    rmSync(stage, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  }
}
