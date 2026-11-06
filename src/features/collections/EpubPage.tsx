import { useEffect, useRef, useState } from 'react';
import type { EpubDocument } from './api';

export function EpubPage({
  book,
  title,
  onKey,
  fitWidth,
  fragment = null,
  navigationSequence = 0,
  onDestination,
}: {
  book: EpubDocument;
  title: string;
  onKey: (key: string) => void;
  fitWidth: boolean;
  fragment?: string | null;
  navigationSequence?: number;
  onDestination?: (found: boolean) => void;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: book.width, height: book.height, scale: 1 });
  const [loaded, setLoaded] = useState(0);
  const destinationCallback = useRef(onDestination);
  destinationCallback.current = onDestination;
  useEffect(() => {
    if (!loaded || !navigationSequence) return;
    const document = frame.current?.contentDocument;
    if (!document) return;
    let active = true;
    void document.fonts.ready.then(() => {
      if (!active) return;
      const target = fragment ? document.getElementById(fragment) : null;
      if (target) target.scrollIntoView({ block: 'start' });
      else document.defaultView?.scrollTo(0, 0);
      frame.current?.focus();
      destinationCallback.current?.(!fragment || !!target);
    });
    // The caller's callback can change while reading. Only an explicit jump,
    // a new document or its load should reset the user's scroll position.
    return () => {
      active = false;
    };
  }, [loaded, book, fragment, navigationSequence]);
  useEffect(() => {
    const viewport = container.current?.parentElement;
    if (!viewport) return;
    const observer = new ResizeObserver(() => {
      const style = getComputedStyle(viewport);
      const width = Math.max(
        1,
        Math.min(
          1000,
          viewport.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight),
        ),
      );
      const height = Math.max(
        1,
        viewport.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom),
      );
      const scale = book.fixed
        ? Math.min(1, width / book.width, fitWidth ? Infinity : height / book.height)
        : 1;
      const next = {
        width: book.fixed ? book.width : width,
        height: book.fixed ? book.height : height,
        scale,
      };
      setSize((current) =>
        current.width === next.width &&
        current.height === next.height &&
        current.scale === next.scale
          ? current
          : next,
      );
    });
    observer.observe(viewport);
    return () => observer.disconnect();
  }, [book, fitWidth]);
  useEffect(() => {
    if (!loaded) return;
    const document = frame.current?.contentDocument;
    if (!document) return;
    const keydown = (event: KeyboardEvent) => {
      const view = document.defaultView;
      if (!book.fixed && view && (event.key === 'PageDown' || event.key === 'PageUp')) {
        const remaining = document.documentElement.scrollHeight - view.innerHeight - view.scrollY;
        if (
          (event.key === 'PageDown' && remaining > 1) ||
          (event.key === 'PageUp' && view.scrollY > 1)
        )
          return;
      }
      if (['ArrowLeft', 'ArrowRight', 'PageDown', 'PageUp', 'Escape'].includes(event.key)) {
        event.preventDefault();
        onKey(event.key);
      }
    };
    document.addEventListener('keydown', keydown);
    return () => {
      document.removeEventListener('keydown', keydown);
    };
  }, [loaded, book, onKey]);
  return (
    <div ref={container} className="collection-epub">
      <div style={{ height: size.height * size.scale, width: size.width * size.scale }}>
        <iframe
          ref={frame}
          title={title}
          sandbox="allow-same-origin"
          referrerPolicy="no-referrer"
          srcDoc={book.html}
          scrolling={book.fixed ? 'no' : 'auto'}
          onLoad={() => setLoaded((value) => value + 1)}
          style={{ width: size.width, height: size.height, transform: `scale(${size.scale})` }}
        />
      </div>
    </div>
  );
}
