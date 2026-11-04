export function Brand({ wordmark = false }: { wordmark?: boolean }) {
  return (
    <span className="brand">
      <svg width="28" height="30" viewBox="0 0 32 34" aria-hidden="true">
        <g fill="none" stroke="currentColor" strokeWidth="1.55" strokeLinejoin="round">
          <path d="M16 2 30 29 2 29Z" />
          <path d="M16 2v19l14 8M16 21 2 29" />
        </g>
      </svg>
      {wordmark ? <span>Prism</span> : null}
    </span>
  );
}
