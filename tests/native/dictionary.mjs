// Generated CC0 fixture packs exercise all four languages without external requests.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-dictionary-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    shortcut: 'Ctrl+Alt+Shift+F8',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
  }),
);
const entries = [
  ['ko', '사과'],
  ['en', 'apple'],
  ['zh', '苹果'],
  ['ja', 'りんご'],
];
let instance;
try {
  instance = await launch(directory);
  const { main, quick, invoke } = instance;
  for (const [language, term] of entries) {
    const path = join(directory, `${language}.jsonl`);
    const manifest = {
      schemaVersion: 1,
      language,
      name: `Synthetic ${language}`,
      version: '1',
      sourceUrl: 'https://example.org/synthetic-dictionary',
      license: 'CC0',
      licenseUrl: 'https://creativecommons.org/publicdomain/zero/1.0/',
      definitionLanguage: 'en',
    };
    await writeFile(
      path,
      `${JSON.stringify({ prismDictionary: manifest })}\n${JSON.stringify({ term, definitions: [`Synthetic ${language} apple definition`] })}\n`,
    );
    await invoke('dictionary_import', { path, manifest: null });
  }
  assert.equal((await invoke('dictionary_packs')).length, 4);
  const input = main.getByRole('textbox', { name: '검색어', exact: true });
  for (const [language, term] of entries) {
    await input.fill(`dict:${language} ${term}`);
    await main.getByRole('button', { name: '오프라인 조회', exact: true }).click();
    await main.getByText(`Synthetic ${language} apple definition`, { exact: true }).waitFor();
    assert((await main.locator('.dictionary-panel').innerText()).includes('기기 내 사전'));
  }
  await main.screenshot({ path: join(evidence, 'dictionary-main.png') });
  await main.getByRole('button', { name: '빠른 검색', exact: true }).click();
  await quick.getByRole('button', { name: '오프라인 조회', exact: true }).click();
  await quick.getByText('Synthetic ja apple definition', { exact: true }).waitFor();
  await quick.screenshot({ path: join(evidence, 'dictionary-quick.png') });
  await quick.getByRole('textbox', { name: '검색어', exact: true }).fill(' ');
  assert.equal(await quick.locator('.quick-results').count(), 0);
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(directory);
  await until(
    async () => (await instance.invoke('dictionary_packs')).length === 4,
    'offline packs survive restart',
  );
  const result = await instance.invoke('dictionary_lookup', {
    language: 'ko',
    word: '사과',
    offline: true,
  });
  assert.equal(result.source, 'offline');
  assert.equal(result.definitions[0].text, 'Synthetic ko apple definition');
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      evidence,
      languages: entries.map(([language]) => language),
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
