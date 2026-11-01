import { useCallback, useEffect, useState, type Dispatch } from 'react';
import { api } from '../api/client';
import { errorMessage } from '../api/errors';
import { readOnboardingDone } from '../state/onboarding';
import type { AppAction } from '../state/reducer';

interface UseBootstrapResult {
  retry: () => void;
}

export function useBootstrap(dispatch: Dispatch<AppAction>): UseBootstrapResult {
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    dispatch({ type: 'bootstrap-loading' });
    void api
      .getBootstrap()
      .then((bootstrap) => {
        if (cancelled) {
          return;
        }
        dispatch({
          type: 'bootstrap-loaded',
          bootstrap,
          onboardingDone: readOnboardingDone(),
        });
      })
      .catch((error: unknown) => {
        if (cancelled) {
          return;
        }
        dispatch({
          type: 'bootstrap-failed',
          error: errorMessage(error),
        });
      });
    return () => {
      cancelled = true;
    };
  }, [attempt, dispatch]);

  const retry = useCallback(() => {
    setAttempt((current) => current + 1);
  }, []);

  return { retry };
}
