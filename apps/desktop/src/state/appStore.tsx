
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { api } from '../api/client';
import { errorMessage } from '../api/errors';
import type {
  AppNotification,
  AppSettings,
  BackendEventMap,
  BackendEventName,
  Layout,
  NotificationSeverity,
  PermissionKind,
  UpdateStatusView,
} from '../api/types';
import { createTranslator, localeFor, type Translate } from '../i18n';
import { clearOnboardingDone, markOnboardingDone } from './onboarding';
import { appReducer, createInitialState, type AppView } from './reducer';
import { useBootstrap } from '../hooks/useBootstrap';

export interface AppContextValue {
  state: ReturnType<typeof createInitialState>;
  t: Translate;
  language: 'zh' | 'en';
  locale: string;
  resolvedTheme: 'light' | 'dark';
  isMock: boolean;
  navigate: (view: AppView) => void;
  notify: (severity: NotificationSeverity, title: string, body?: string) => void;
  dismissToast: (id: number) => void;
  reportError: (error: unknown, title?: string) => void;
  retry: () => void;
  saveSettings: (patch: Partial<AppSettings>) => Promise<AppSettings | null>;
  markNotificationRead: (id: number) => Promise<void>;
  markAllNotificationsRead: () => Promise<void>;
  clearNotifications: () => Promise<void>;
  runNotificationAction: (notification: AppNotification) => Promise<void>;
  requestPermission: (kind: PermissionKind) => Promise<boolean>;
  openPermissionSettings: (kind: PermissionKind) => Promise<void>;
  closePairing: () => void;
  completeOnboarding: () => void;
  openOnboarding: () => void;
  applyLayout: (layout: Layout) => void;
  refreshUpdateStatus: () => Promise<UpdateStatusView | null>;
}

const AppContext = createContext<AppContextValue | null>(null);

function getSystemPrefersDark(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
    return false;
  }
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(appReducer, undefined, createInitialState);
  const language = state.settings.language;
  const t = useMemo(() => createTranslator(language), [language]);
  const locale = localeFor(language);
  const languageRef = useRef(language);
  languageRef.current = language;
  const translateRef = useRef(t);
  translateRef.current = t;

  const [systemPrefersDark, setSystemPrefersDark] = useState(getSystemPrefersDark);
  const resolvedTheme: 'light' | 'dark' =
    state.settings.theme === 'system'
      ? systemPrefersDark
        ? 'dark'
        : 'light'
      : state.settings.theme;

  const toastSequence = useRef(0);
  const toastTimers = useRef(new Map<number, ReturnType<typeof setTimeout>>());
  const settingsRef = useRef(state.settings);
  const settingsQueue = useRef<Promise<unknown>>(Promise.resolve());

  useEffect(() => {
    settingsRef.current = state.settings;
  }, [state.settings]);

  useEffect(() => {
    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
    const handleChange = (event: MediaQueryListEvent) => {
      setSystemPrefersDark(event.matches);
    };
    setSystemPrefersDark(mediaQuery.matches);
    mediaQuery.addEventListener('change', handleChange);
    return () => {
      mediaQuery.removeEventListener('change', handleChange);
    };
  }, []);

  useEffect(() => {
    if (api.isMock()) {
      return;
    }
    const refreshPermissions = () => {
      void api
        .getPermissions()
        .then((permissions) => dispatch({ type: 'permissions-changed', permissions }))
        .catch(() => undefined);
    };
    window.addEventListener('focus', refreshPermissions);
    return () => window.removeEventListener('focus', refreshPermissions);
  }, []);

  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = resolvedTheme;
    root.style.colorScheme = resolvedTheme;
    root.lang = language === 'zh' ? 'zh-CN' : 'en';
    document
      .querySelector('meta[name="theme-color"]')
      ?.setAttribute('content', resolvedTheme === 'dark' ? '#0b0b0f' : '#f6f5f2');
    try {
      window.localStorage.setItem('nexuscat.theme', state.settings.theme);
    } catch {
    }
  }, [resolvedTheme, language, state.settings.theme]);

  useEffect(() => {
    return () => {
      toastTimers.current.forEach((timer) => clearTimeout(timer));
      toastTimers.current.clear();
    };
  }, []);

  const dismissToast = useCallback((id: number) => {
    const timer = toastTimers.current.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      toastTimers.current.delete(id);
    }
    dispatch({ type: 'toast-dismissed', id });
  }, []);

  const notify = useCallback(
    (severity: NotificationSeverity, title: string, body?: string) => {
      const id = ++toastSequence.current;
      dispatch({
        type: 'toast-added',
        toast: {
          id,
          severity,
          title,
          ...(body ? { body } : {}),
        },
      });
      const duration =
        severity === 'error' || severity === 'warning' ? 8000 : 5200;
      const timer = setTimeout(() => {
        toastTimers.current.delete(id);
        dispatch({ type: 'toast-dismissed', id });
      }, duration);
      toastTimers.current.set(id, timer);
    },
    [],
  );

  const reportError = useCallback(
    (error: unknown, title?: string) => {
      const detail = errorMessage(error) || translateRef.current('error.unknown');
      notify(
        'error',
        title ?? translateRef.current('toast.errorTitle'),
        detail,
      );
    },
    [notify],
  );

  const navigate = useCallback((view: AppView) => {
    dispatch({ type: 'navigate', view });
  }, []);

  const saveSettings = useCallback(
    (patch: Partial<AppSettings>): Promise<AppSettings | null> => {
      const previous = settingsRef.current;
      const next: AppSettings = {
        ...previous,
        ...patch,
        target: patch.target
          ? { ...previous.target, ...patch.target }
          : previous.target,
        shortcuts: patch.shortcuts
          ? { ...previous.shortcuts, ...patch.shortcuts }
          : previous.shortcuts,
        video_targets: patch.video_targets
          ? { ...previous.video_targets, ...patch.video_targets }
          : previous.video_targets,
      };
      settingsRef.current = next;
      dispatch({ type: 'settings-updated', settings: next });

      const operation = settingsQueue.current.then(async () => {
        const saved = await api.updateSettings(next);
        if (settingsRef.current === next) {
          settingsRef.current = saved;
          dispatch({ type: 'settings-updated', settings: saved });
        }
        return saved;
      });
      settingsQueue.current = operation.catch(() => undefined);

      return operation.catch((error: unknown) => {
        if (settingsRef.current === next) {
          settingsRef.current = previous;
          dispatch({ type: 'settings-updated', settings: previous });
        }
        reportError(error, translateRef.current('settings.saveError'));
        return null;
      });
    },
    [reportError],
  );

  const syncNotifications = useCallback(async () => {
    try {
      const notifications = await api.getNotifications();
      dispatch({ type: 'notifications-replaced', notifications });
    } catch (error) {
      reportError(error);
    }
  }, [reportError]);

  const markNotificationRead = useCallback(
    async (id: number) => {
      dispatch({ type: 'notification-marked-read', id });
      try {
        await api.markNotificationRead(id);
      } catch (error) {
        await syncNotifications();
        reportError(error);
      }
    },
    [reportError, syncNotifications],
  );

  const markAllNotificationsRead = useCallback(async () => {
    dispatch({ type: 'all-notifications-read' });
    try {
      await api.markAllNotificationsRead();
    } catch (error) {
      await syncNotifications();
      reportError(error);
    }
  }, [reportError, syncNotifications]);

  const clearNotifications = useCallback(async () => {
    dispatch({ type: 'notifications-cleared' });
    try {
      await api.clearNotifications();
    } catch (error) {
      await syncNotifications();
      reportError(error);
    }
  }, [reportError, syncNotifications]);

  const runNotificationAction = useCallback(
    async (notification: AppNotification) => {
      if (!notification.read) {
        void markNotificationRead(notification.id);
      }
      const action = notification.action;
      if (!action) {
        return;
      }
      try {
        switch (action.kind) {
          case 'open_control_panel':
            navigate('workbench');
            await api.showControlPanel();
            break;
          case 'open_settings':
            navigate('settings');
            break;
          case 'show_stage':
            await api.summonStage();
            break;
          case 'retry_connection':
            if (action.payload) {
              await api.connectDevice(action.payload);
            } else {
              navigate('workbench');
            }
            break;
          case 'open_update':
            dispatch({ type: 'open-update' });
            break;
          case 'dismiss':
            break;
          default:
            break;
        }
      } catch (error) {
        reportError(error, translateRef.current('toast.notificationActionFailed'));
      }
    },
    [markNotificationRead, navigate, reportError],
  );

  const requestPermission = useCallback(
    async (kind: PermissionKind): Promise<boolean> => {
      try {
        const granted = await api.requestPermission(kind);
        const permissions = await api.getPermissions();
        dispatch({ type: 'permissions-changed', permissions });
        return granted;
      } catch (error) {
        reportError(error, translateRef.current('permissions.requestFailed'));
        return false;
      }
    },
    [reportError],
  );

  const openPermissionSettings = useCallback(
    async (kind: PermissionKind) => {
      try {
        await api.openPermissionSettings(kind);
      } catch (error) {
        reportError(error, translateRef.current('permissions.requestFailed'));
      }
    },
    [reportError],
  );

  const closePairing = useCallback(() => {
    dispatch({ type: 'pairing-closed' });
  }, []);

  const applyLayout = useCallback((layout: Layout) => {
    dispatch({ type: 'layout-changed', layout });
  }, []);

  const refreshUpdateStatus = useCallback(async (): Promise<UpdateStatusView | null> => {
    try {
      const status = await api.getUpdateStatus();
      dispatch({ type: 'update-status', status });
      return status;
    } catch (error) {
      reportError(error);
      return null;
    }
  }, [reportError]);

  const completeOnboarding = useCallback(() => {
    markOnboardingDone();
    dispatch({ type: 'onboarding-completed' });
    void api.hideToBackground().catch((error: unknown) => reportError(error));
  }, [reportError]);

  const openOnboarding = useCallback(() => {
    clearOnboardingDone();
    dispatch({ type: 'onboarding-opened' });
  }, []);

  const { retry } = useBootstrap(dispatch);

  useEffect(() => {
    let disposed = false;
    let subscriptionErrorShown = false;
    const cleanups: Array<() => void> = [];

    function bind<K extends BackendEventName>(
      event: K,
      handler: (payload: BackendEventMap[K]) => void,
    ): void {
      void api
        .subscribe(event, handler)
        .then((unlisten) => {
          if (disposed) {
            unlisten();
          } else {
            cleanups.push(unlisten);
          }
        })
        .catch((error: unknown) => {
          if (!disposed && !subscriptionErrorShown) {
            subscriptionErrorShown = true;
            reportError(error);
          }
        });
    }

    bind('session-changed', (session) => {
      dispatch({ type: 'session-changed', session });
    });
    bind('devices-changed', (devices) => {
      dispatch({ type: 'devices-changed', devices });
    });
    bind('layout-changed', (layout) => {
      dispatch({ type: 'layout-changed', layout });
    });
    bind('settings-changed', (settings) => {
      settingsRef.current = settings;
      dispatch({ type: 'settings-updated', settings });
    });
    bind('shared-sync-status', (status) => {
      dispatch({ type: 'shared-sync-status', status });
    });
    bind('notification', (notification) => {
      dispatch({ type: 'notification-received', notification });
    });
    bind('notifications-changed', (notifications) => {
      dispatch({ type: 'notifications-replaced', notifications });
    });
    bind('pairing-event', (event) => {
      dispatch({ type: 'pairing-event', event });
    });
    bind('metrics', (metrics) => {
      dispatch({ type: 'metrics', metrics });
    });
    bind('transfer-progress', (transfer) => {
      dispatch({ type: 'transfer-progress', transfer });
    });
    bind('transfers-changed', (transfers) => {
      dispatch({ type: 'transfers-replaced', transfers });
    });
    bind('update-status', (status) => {
      dispatch({ type: 'update-status', status });
      if (status.state === 'failed') {
        notify(
          'error',
          translateRef.current('updates.title'),
          status.message ?? translateRef.current('updates.state.failed'),
        );
      }
    });
    bind('permission-changed', (permissions) => {
      dispatch({ type: 'permissions-changed', permissions });
    });
    bind('group-status', (status) => {
      dispatch({ type: 'group-status', status });
    });
    bind('media-restart', (info) => {
      dispatch({ type: 'media-restart', info });
    });
    bind('stage-command', (payload) => {
      dispatch({ type: 'stage-command', action: payload.action });
    });
    bind('calibration-open', () => {
      dispatch({ type: 'navigate', view: 'workbench' });
    });
    bind('open-workbench', () => {
      dispatch({ type: 'navigate', view: 'workbench' });
    });
    bind('open-settings', () => {
      dispatch({ type: 'navigate', view: 'settings' });
    });
    bind('open-update', () => {
      dispatch({ type: 'open-update' });
    });
    bind('request-quit', () => {
      void settingsQueue.current.finally(() => {
        api.quitApp().catch((error: unknown) => {
          reportError(error);
        });
      });
    });

    return () => {
      disposed = true;
      cleanups.forEach((unlisten) => unlisten());
    };
  }, [notify, reportError]);

  const contextValue: AppContextValue = {
    state,
    t,
    language,
    locale,
    resolvedTheme,
    isMock: api.isMock(),
    navigate,
    notify,
    dismissToast,
    reportError,
    retry,
    saveSettings,
    markNotificationRead,
    markAllNotificationsRead,
    clearNotifications,
    runNotificationAction,
    requestPermission,
    openPermissionSettings,
    closePairing,
    completeOnboarding,
    openOnboarding,
    applyLayout,
    refreshUpdateStatus,
  };

  return <AppContext.Provider value={contextValue}>{children}</AppContext.Provider>;
}

export function useApp(): AppContextValue {
  const value = useContext(AppContext);
  if (!value) {
    throw new Error('useApp must be used inside AppProvider');
  }
  return value;
}
