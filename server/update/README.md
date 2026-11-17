# Prism update server

Two files on any web host with PHP 7.4 or newer:

| file          | what it is                                                                    |
| ------------- | ----------------------------------------------------------------------------- |
| `index.php`   | answers Prism's update check; checks `latest.json` before sending it          |
| `latest.json` | the latest version, and for each platform the package URL, SHA-256, signature |

No database, framework or other PHP files. `latest.example.json` shows the format.

## How Prism uses it

1. Prism is built with the server address:
   `PRISM_UPDATE_URL=https://example.com/prism/update/index.php`.
   It is fixed in the app; users cannot point Prism at another server. A build without
   it says that no update server is configured. `PRISM_DOWNLOAD_PAGE` optionally sets
   the page opened for manual downloads.
2. Prism asks that URL (HTTPS only) and compares `version` with its own version.
3. For a newer version it downloads the package from its platform's `url`, checks the
   `sha256` hash, then the `signature` with the public key built into Prism, and only
   then installs.

The hash catches a damaged or wrong download. The signature proves the package is a
real Prism build: the hash comes from the same server as the package, so it cannot do
that on its own. Keep the signing private key off the server.

## Publishing a version

1. Build and sign the packages (release workflow, `TAURI_SIGNING_PRIVATE_KEY`).
2. Make `latest.json` from the signed files. Point the package URLs at your server:

   ```sh
   PRISM_UPDATER_PUBLIC_KEY="$(cat public.key)" node scripts/make-latest-json.mjs \
     --dir out --version 0.2.0 --base-url https://example.com/prism/files \
     --notes "Prism 0.2.0"
   ```

   It verifies every package signature and version, computes the SHA-256 hashes and
   refuses to overwrite an existing `latest.json`. `--allow-partial` permits missing
   platforms.

3. Upload the packages to the `--base-url` folder, then replace `latest.json` next to
   `index.php`. Upload `latest.json` last, so it never names files that are not there yet.

`index.php` refuses a broken `latest.json` (bad version, non-HTTPS URL, malformed hash,
missing signature, over 64 KiB) with `503` and logs the reason with `error_log`. Prism
then simply reports that update information is unavailable.

## Test

`node --test tests/scripts/update-server.test.mjs` starts the real `index.php` with
`php -S` (skipped when `php` is not installed).
