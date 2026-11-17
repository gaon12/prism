<?php
// Prism update server: one small PHP file and one data file, nothing else.
//
// Prism asks this URL (set when Prism is built, PRISM_UPDATE_URL) for the latest version.
// This script reads `latest.json` from the same folder, checks that it is complete, and
// returns it as JSON. Prism then compares the version, downloads the package from `url`,
// checks its SHA-256 hash (`sha256`) and its signature (`signature`), and only then installs.
//
// Publishing a new version = replacing `latest.json` (made by scripts/make-latest-json.mjs)
// and uploading the package files it points to. No database, no other PHP files.
//
// Requires PHP 7.4 or newer.

declare(strict_types=1);

// Largest data file accepted. Prism refuses update information above 64 KiB as well.
const MAX_BYTES = 65536;

/** Sends a JSON response and stops. */
function respond(int $status, array $body): void
{
    http_response_code($status);
    header('Content-Type: application/json; charset=utf-8');
    // Always ask again: a stale cached answer would hide a new version.
    header('Cache-Control: no-store');
    header('X-Content-Type-Options: nosniff');
    echo json_encode($body, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_PRETTY_PRINT);
    exit;
}

/** Reports a broken data file without revealing server paths or PHP details. */
function unavailable(string $reason): void
{
    error_log('Prism update server: ' . $reason);
    respond(503, ['error' => 'Update information is temporarily unavailable.']);
}

// Only reading is supported.
$method = $_SERVER['REQUEST_METHOD'] ?? 'GET';
if ($method !== 'GET' && $method !== 'HEAD') {
    header('Allow: GET, HEAD');
    respond(405, ['error' => 'Only GET is supported.']);
}

// 1. Read the data file next to this script.
$path = __DIR__ . DIRECTORY_SEPARATOR . 'latest.json';
if (!is_file($path) || is_link($path)) {
    unavailable('latest.json is missing.');
}
$size = filesize($path);
if ($size === false || $size === 0 || $size > MAX_BYTES) {
    unavailable('latest.json is empty or larger than 64 KiB.');
}
$text = file_get_contents($path);
$data = json_decode((string) $text, true);
if (!is_array($data)) {
    unavailable('latest.json is not valid JSON.');
}

// 2. Check every field Prism relies on, so a typo is caught here and not on users' PCs.
$semver = '/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/';
$version = $data['version'] ?? null;
if (!is_string($version) || strlen($version) > 128 || !preg_match($semver, $version)) {
    unavailable('version must be a version number such as 1.2.3.');
}
$notes = $data['notes'] ?? '';
if (!is_string($notes) || strlen($notes) > 8000) {
    unavailable('notes must be text of at most 8000 bytes.');
}
$pubDate = $data['pub_date'] ?? null;
if (!is_string($pubDate) || strtotime($pubDate) === false) {
    unavailable('pub_date must be a date such as 2026-11-17T00:00:00.000Z.');
}
$platforms = $data['platforms'] ?? null;
if (!is_array($platforms) || $platforms === [] || array_is_list_compat($platforms)) {
    unavailable('platforms must name at least one platform.');
}
$checked = [];
foreach ($platforms as $platform => $entry) {
    // Platform names look like "windows-x86_64" or "linux-aarch64".
    if (!is_string($platform) || !preg_match('/^[a-z0-9]+-[a-z0-9_]+$/', $platform)) {
        unavailable('a platform name is invalid.');
    }
    $url = is_array($entry) ? ($entry['url'] ?? null) : null;
    $sha256 = is_array($entry) ? ($entry['sha256'] ?? null) : null;
    $signature = is_array($entry) ? ($entry['signature'] ?? null) : null;
    // Prism downloads only over HTTPS, without user names, passwords or fragments.
    $parts = is_string($url) ? parse_url($url) : false;
    if (
        !is_array($parts)
        || ($parts['scheme'] ?? '') !== 'https'
        || ($parts['host'] ?? '') === ''
        || isset($parts['user'])
        || isset($parts['pass'])
        || isset($parts['fragment'])
    ) {
        unavailable("$platform: url must be an https:// address.");
    }
    if (!is_string($sha256) || !preg_match('/^[0-9a-f]{64}$/', $sha256)) {
        unavailable("$platform: sha256 must be 64 lowercase hexadecimal characters.");
    }
    if (!is_string($signature) || trim($signature) === '' || strlen($signature) > 16384) {
        unavailable("$platform: signature is missing.");
    }
    $checked[$platform] = ['url' => $url, 'sha256' => $sha256, 'signature' => $signature];
}

// 3. Answer with exactly the checked fields (anything else in the file is left out).
respond(200, [
    'version' => $version,
    'notes' => $notes,
    'pub_date' => $pubDate,
    'platforms' => $checked,
]);

/** True for a plain list (`[...]`) instead of an object of named platforms; PHP 7.4-safe. */
function array_is_list_compat(array $value): bool
{
    return $value === [] || array_keys($value) === range(0, count($value) - 1);
}
