import type { ReactNode } from 'react';
export function EmptyState({
  icon,
  title,
  children,
  actions,
}: {
  icon: ReactNode;
  title: string;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="empty-state">
      <div className="empty-icon">{icon}</div>
      <h2>{title}</h2>
      <div className="empty-copy">{children}</div>
      {actions ? <div className="empty-actions">{actions}</div> : null}
    </div>
  );
}
