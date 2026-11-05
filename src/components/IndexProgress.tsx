import './indexing.css';

/** Traversal is streamed, so its final denominator is unknown. Do not invent %. */
export function IndexProgress({ active, label }: { active: boolean; label: string }) {
  return (
    <div className="index-progress" data-active={active} aria-hidden={!active}>
      <progress aria-label={label} {...(!active ? { value: 1 } : {})} />
    </div>
  );
}
