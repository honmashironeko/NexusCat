import { notificationSeverityIcon } from '../domain/labels';
import { useApp } from '../state/appStore';
import { Icon } from './Icon';

export function Toaster() {
  const { state, t, dismissToast } = useApp();
  if (state.toasts.length === 0) {
    return null;
  }
  return (
    <div className="toast-region" aria-live="polite" aria-atomic="false">
      {state.toasts.map((toast) => (
        <div
          key={toast.id}
          className={`toast toast-${toast.severity}`}
          role={toast.severity === 'error' || toast.severity === 'warning' ? 'alert' : 'status'}
        >
          <span className="toast-icon" aria-hidden="true">
            <Icon name={notificationSeverityIcon(toast.severity)} size={17} />
          </span>
          <div className="toast-copy">
            <strong>{toast.title}</strong>
            {toast.body ? <span>{toast.body}</span> : null}
          </div>
          <button
            type="button"
            className="toast-close"
            aria-label={t('common.dismiss')}
            onClick={() => dismissToast(toast.id)}
          >
            <Icon name="close" size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
