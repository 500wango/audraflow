import { useI18n } from '../useI18n';
import type { Page } from '../types';

export function AppNavigation({
  currentPage,
  jobsCount,
  onNavigate,
  className = '',
}: {
  currentPage: Page;
  jobsCount: number;
  onNavigate: (page: Page) => void;
  className?: string;
}) {
  const { t } = useI18n();
  const navItems: { page: Page; label: string; badge?: number }[] = [
    { page: 'import', label: t('nav.import') },
    { page: 'queue', label: t('nav.queue'), badge: jobsCount > 0 ? jobsCount : undefined },
    { page: 'editor', label: t('nav.editor') },
    { page: 'export', label: t('nav.export') },
    { page: 'settings', label: t('nav.settings') },
  ];

  return (
    <nav className={`app-nav ${className}`.trim()} aria-label={t('nav.primary')}>
      {navItems.map((item, index) => (
        <button
          key={item.page}
          type="button"
          className={currentPage === item.page ? 'active' : ''}
          onClick={() => onNavigate(item.page)}
        >
          <span className="nav-index">{String(index + 1).padStart(2, '0')}</span>
          <span className="nav-label">{item.label}</span>
          {item.badge !== undefined && (
            <span className="badge">{item.badge}</span>
          )}
        </button>
      ))}
    </nav>
  );
}
