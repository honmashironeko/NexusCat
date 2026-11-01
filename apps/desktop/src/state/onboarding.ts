const ONBOARDING_STORAGE_KEY = 'nexuscat.onboarding.v1';

export function readOnboardingDone(): boolean {
  try {
    return window.localStorage.getItem(ONBOARDING_STORAGE_KEY) === 'done';
  } catch {
    return false;
  }
}

export function markOnboardingDone(): void {
  try {
    window.localStorage.setItem(ONBOARDING_STORAGE_KEY, 'done');
  } catch {
  }
}

export function clearOnboardingDone(): void {
  try {
    window.localStorage.removeItem(ONBOARDING_STORAGE_KEY);
  } catch {
  }
}
