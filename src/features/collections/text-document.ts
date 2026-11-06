import type { EpubDocument } from './api';

/** The reader and its thumbnail use the same inert, reflowable text page.
 * Plain text has no author page layout; Office extraction remains a labelled
 * text preview, never a reconstruction of its original page formatting.
 */
export function textDocument(text: string): EpubDocument {
  if (text.length > 10_000) throw { code: 'limit' };
  const escaped = text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  return {
    width: 832,
    height: 1176,
    fixed: false,
    html: `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"><style>
html { color-scheme: light; background: #fff; color: #202124; }
body { margin: 0; padding: 36px 46px; }
pre { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere;
  font: 17.6px/1.95 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", "Malgun Gothic", sans-serif; }
@media (max-width: 600px) { body { padding: 20px; } }
</style></head><body><pre>${escaped}</pre></body></html>`,
  };
}
