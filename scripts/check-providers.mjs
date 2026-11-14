// Runs the hosted provider check (tools/platform-qa/src/external_providers.rs)
// on this computer, with API keys taken from a local, git-ignored env file.
//
//   npm run test:providers                      every provider with a key
//   npm run test:providers -- groq-gpt-oss-20b  only the named providers
//
// The env file (default `.env.txt` in the project root, or PRISM_PROVIDER_ENV)
// holds lines like `LLM_API_GROQ=...`. Each LLM_API_<NAME> becomes the
// <NAME>_API_KEY variable the check reads, for the cargo process only.
// Key values are never printed; only the names of the keys found are.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const envFile = resolve(root, process.env.PRISM_PROVIDER_ENV ?? '.env.txt');

/** Reads `NAME=value` lines; blank lines and `#` comments are skipped. */
function readEnvFile(path) {
  const values = {};
  for (const line of readFileSync(path, 'utf8').split(/\r?\n/)) {
    const match = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$/.exec(line);
    if (!match || line.trimStart().startsWith('#')) continue;
    // Allow optional surrounding quotes: LLM_API_GROQ="..."
    values[match[1]] = match[2].replace(/^(['"])(.*)\1$/, '$2');
  }
  return values;
}

const keys = {};
if (existsSync(envFile)) {
  for (const [name, value] of Object.entries(readEnvFile(envFile))) {
    const provider = /^LLM_API_([A-Z0-9_]+)$/.exec(name)?.[1];
    if (provider && value) keys[`${provider}_API_KEY`] = value;
  }
} else {
  console.log(`No ${envFile}; using keys already in the environment.`);
}
console.log(`Keys found: ${Object.keys(keys).join(', ') || 'none'}`);

const result = spawnSync(
  'cargo',
  [
    'test',
    '--manifest-path',
    'tools/platform-qa/Cargo.toml',
    '--locked',
    '--lib',
    'external_providers',
    '--',
    '--ignored',
    '--nocapture',
    '--test-threads=1',
  ],
  {
    cwd: root,
    stdio: 'inherit',
    env: {
      ...process.env,
      ...keys,
      // Names after `--` limit the run, the same as the workflow input.
      PRISM_PROVIDERS: process.argv.slice(2).join(',') || process.env.PRISM_PROVIDERS || '',
    },
  },
);
process.exit(result.status ?? 1);
