import type { EpubDocument } from './api';

// Render the same inert document used by the reader: native-sanitized EPUB
// or escaped plain/extracted text, with this WebView's HTML/CSS/image engine.
// No server, file URL or invented cover is used.
// The two-task media queue also bounds this work and its native cache write.
export async function epubThumbnail(book: EpubDocument): Promise<string> {
  if (book.html.length > 32 * 1024 ** 2 || book.width < 1 || book.height < 1)
    throw { code: 'limit' };
  const parsed = new DOMParser().parseFromString(book.html, 'text/html');
  // Image SVG has no script execution. Keep all author content/styles in its
  // own XHTML document so book selectors never affect the Prism interface.
  const html = new XMLSerializer().serializeToString(parsed.documentElement);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${book.width}" height="${book.height}"><foreignObject width="100%" height="100%">${html}</foreignObject></svg>`;
  const source = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  const image = new Image();
  const loaded = new Promise<void>((resolve, reject) => {
    const timer = window.setTimeout(() => {
      image.src = '';
      reject({ code: 'limit' });
    }, 15_000);
    image.onload = () => {
      clearTimeout(timer);
      resolve();
    };
    image.onerror = () => {
      clearTimeout(timer);
      reject({ code: 'unsupported' });
    };
  });
  image.src = source;
  await loaded;
  const scale = Math.min(1, 480 / Math.max(book.width, book.height));
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.floor(book.width * scale));
  canvas.height = Math.max(1, Math.floor(book.height * scale));
  const context = canvas.getContext('2d');
  if (!context) throw { code: 'unsupported' };
  context.fillStyle = 'white';
  context.fillRect(0, 0, canvas.width, canvas.height);
  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  // PNG encoding is available in WebKit as well as Chromium. WebKit may
  // silently return PNG for an image/webp request; the native admission path
  // validates this bitmap and always persists a WebP thumbnail.
  const url = canvas.toDataURL('image/png');
  image.src = '';
  canvas.width = canvas.height = 0;
  if (!url.startsWith('data:image/png;base64,')) throw { code: 'unsupported' };
  return url;
}
