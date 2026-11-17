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

## Web server settings

Only the update check and the package files need to be reachable:

```
<site>/prism/update.php        the update check (index.php here; any name works)
<site>/prism/latest.json       read by update.php from disk; refused over the web
<site>/prism/files/Prism_<version>_x64-setup.exe
```

- **Apache:** copy `.htaccess` next to `update.php` and `files/.htaccess` into `files/`. The
  folder needs `AllowOverride AuthConfig FileInfo Indexes Limit` (or `All`), otherwise Apache
  ignores the files or answers 500.
- **nginx:** paste the locations from `nginx.conf` into the site's HTTPS `server { }` block,
  set the PHP-FPM socket, then run `nginx -t` and reload.

Both refuse everything else under `/prism/`: `latest.json`, other PHP files, notes and any key or
certificate copied there by mistake. Folder listings are off, scripts in `files/` never run,
and packages are served as plain downloads with a one-hour cache.

Check after setting up (expected status in brackets):

```sh
curl -I https://api.rasca99.dev/prism/update.php                 # 200, or 503 before latest.json
curl -I https://api.rasca99.dev/prism/latest.json                # 403 (Apache) / 404 (nginx)
curl -I https://api.rasca99.dev/prism/files/                     # 403 / 404, no listing
curl -I https://api.rasca99.dev/prism/files/Prism_0.2.0_x64-setup.exe   # 200 once uploaded
curl -I -X POST https://api.rasca99.dev/prism/update.php         # 405 / 403
```

## Test

`node --test tests/scripts/update-server.test.mjs` starts the real `index.php` with
`php -S` (skipped when `php` is not installed).
