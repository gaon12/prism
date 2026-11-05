/*! Adapted from Stageshow's Segmented, Copyright (c) 2026 Benji Taylor, MIT.
 * Source: https://github.com/benjitaylor/skills — stageshow/kit/controls/Segmented
 * See LICENSE.stageshow. Live buttons/keyboard behavior replace authored-demo spans.
 */
import { useId, useRef } from 'react';
import type { CSSProperties, KeyboardEvent } from 'react';
import './segmented.css';

export function Segmented<T extends string>({
  items,
  value,
  onChange,
  label,
  className = '',
}: {
  items: readonly { value: T; label: string; description?: string }[];
  value: T | null;
  onChange: (value: T) => void;
  label: string;
  className?: string;
}) {
  const id = useId();
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const selected = items.findIndex((item) => item.value === value);
  const keyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? items.length - 1
          : event.key === 'ArrowRight'
            ? (index + 1) % items.length
            : event.key === 'ArrowLeft'
              ? (index - 1 + items.length) % items.length
              : null;
    if (next === null) return;
    event.preventDefault();
    event.stopPropagation();
    buttons.current[next]?.focus();
    onChange(items[next].value);
  };
  return (
    <div
      role="group"
      aria-label={label}
      className={`prism-segmented ss-seg ${className}`}
      style={{ '--n': items.length } as CSSProperties}
    >
      <span
        className="ss-seg__thumb"
        aria-hidden="true"
        data-empty={selected < 0 || undefined}
        style={{ transform: `translateX(${Math.max(selected, 0) * 100}%)` }}
      />
      {items.map((item, index) => (
        <button
          key={item.value}
          type="button"
          ref={(element) => {
            buttons.current[index] = element;
          }}
          className={`ss-seg__option${index === selected ? ' active' : ''}`}
          aria-pressed={index === selected}
          tabIndex={index === Math.max(selected, 0) ? 0 : -1}
          title={item.description}
          aria-describedby={item.description ? `${id}-${index}` : undefined}
          onClick={() => onChange(item.value)}
          onKeyDown={(event) => keyDown(event, index)}
        >
          {item.label}
        </button>
      ))}
      {items.map((item, index) =>
        item.description ? (
          <span key={item.value} id={`${id}-${index}`} className="ss-seg__description">
            {item.description}
          </span>
        ) : null,
      )}
    </div>
  );
}
