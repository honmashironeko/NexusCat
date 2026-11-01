import { useEffect, useRef, useState } from 'react';
import type { AppNotification } from '../api/types';
import { formatRelativeTime } from '../domain/format';
import {
  notificationActionKey,
  notificationCategoryKey,
  notificationSeverityIcon,
} from '../domain/labels';
import { useApp } from '../state/appStore';
import { Icon } from './Icon';

export function NotificationCenter() {
  const {
    state,
    t,
    language,
    markNotificationRead,
    markAllNotificationsRead,
    clearNotifications,
    runNotificationAction,
  } = useApp();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const unread = state.notifications.reduce(
    (count, notification) => (notification.read ? count : count + 1),
    0,
  );

  useEffect(() => {
    if (!open) {
      return;
    }
    const handlePointerDown = (event: PointerEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
      }
    };
    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [open]);

  const runAction = async (notification: AppNotification) => {
    setOpen(false);
    await runNotificationAction(notification);
  };

  return (
    <div className="notification-center" ref={wrapRef}>
      <button
        type="button"
        className="icon-button notification-bell"
        aria-label={t('notifications.bellAria')}
        aria-expanded={open}
        title={t('notifications.bellAria')}
        onClick={() => setOpen((value) => !value)}
      >
        <Icon name="bell" size={14} />
        {unread > 0 ? (
          <span className="notification-badge" aria-hidden="true">
            {unread > 99 ? '99+' : unread}
          </span>
        ) : null}
      </button>

      {open ? (
        <div
          className="notification-popover"
          role="dialog"
          aria-label={t('notifications.popoverAria')}
        >
          <header className="notification-head">
            <div className="notification-heading">
              <strong>{t('notifications.title')}</strong>
              <span>{t('notifications.unreadCount', { count: unread })}</span>
            </div>
            <div className="notification-head-actions">
              <button
                type="button"
                className="text-button"
                disabled={unread === 0}
                onClick={() => void markAllNotificationsRead()}
              >
                {t('notifications.markAllRead')}
              </button>
              <button
                type="button"
                className="text-button danger"
                disabled={state.notifications.length === 0}
                onClick={() => void clearNotifications()}
              >
                {t('notifications.clear')}
              </button>
            </div>
          </header>

          {state.notifications.length === 0 ? (
            <p className="notification-empty">{t('notifications.empty')}</p>
          ) : (
            <ul className="notification-list">
              {state.notifications.map((notification) => (
                <li
                  key={notification.id}
                  className={
                    notification.read
                      ? 'notification-item'
                      : 'notification-item is-unread'
                  }
                >
                  <button
                    type="button"
                    className="notification-main"
                    onClick={() => void runAction(notification)}
                  >
                    <span
                      className={`notification-icon severity-${notification.severity}`}
                      aria-hidden="true"
                    >
                      <Icon
                        name={notificationSeverityIcon(notification.severity)}
                        size={13}
                      />
                    </span>
                    <span className="notification-copy">
                      <span className="notification-title">
                        {notification.title}
                        {notification.read ? null : (
                          <i className="notification-dot" aria-hidden="true" />
                        )}
                      </span>
                      {notification.body ? (
                        <span className="notification-body">{notification.body}</span>
                      ) : null}
                      <span className="notification-meta">
                        {t(notificationCategoryKey(notification.category))}
                        <span aria-hidden="true"> · </span>
                        {formatRelativeTime(notification.timestamp_ms, language)}
                      </span>
                    </span>
                  </button>
                  {notification.action ? (
                    <button
                      type="button"
                      className="btn compact notification-action"
                      onClick={() => void runAction(notification)}
                    >
                      {notification.action.label ||
                        t(notificationActionKey(notification.action.kind))}
                    </button>
                  ) : null}
                  {notification.read ? null : (
                    <button
                      type="button"
                      className="notification-read"
                      aria-label={t('notifications.markReadAria', {
                        title: notification.title,
                      })}
                      title={t('notifications.markRead')}
                      onClick={() => void markNotificationRead(notification.id)}
                    >
                      <Icon name="check" size={11} />
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </div>
  );
}
