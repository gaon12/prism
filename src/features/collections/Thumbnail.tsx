import { useEffect, useRef, useState } from 'react';
import { BookOpen, FileImage } from 'lucide-react';
import type { Language } from '../../lib/types';
import { library, mediaTask } from './api';
import { libraryText } from './messages';
export function Thumbnail({
  id,
  kind,
  revision,
  language,
}: {
  id: string;
  kind: 'work' | 'page';
  revision: number;
  language: Language;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [image, setImage] = useState<{ key: string; src: string | null } | null>(null);
  const key = `${kind}:${id}:${revision}`;
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: '200px' },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!visible) return;
    let disposed = false;
    void mediaTask(
      () => (kind === 'work' ? library.cover(id) : library.media(id, true)),
      () => disposed,
    ).then(
      (media) => {
        if (!disposed) setImage({ key, src: media.dataUrl });
      },
      () => {
        if (!disposed) setImage({ key, src: null });
      },
    );
    return () => {
      disposed = true;
    };
  }, [key, visible, kind, id]);
  const current = image?.key === key ? image.src : null;
  return (
    <span ref={ref} className="collection-thumbnail">
      {current ? (
        <img src={current} alt="" draggable={false} />
      ) : (
        <span className="collection-thumbnail-empty">
          {kind === 'work' ? <BookOpen size={30} /> : <FileImage size={24} />}
          <small>{libraryText(language, image?.key === key ? 'noPreview' : 'loading')}</small>
        </span>
      )}
    </span>
  );
}
