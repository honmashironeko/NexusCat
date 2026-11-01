import { useEffect, useRef, useState } from 'react';

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tagName = target.tagName.toLowerCase();
  if (tagName === 'input' || tagName === 'textarea' || tagName === 'select') {
    return true;
  }
  return target.isContentEditable || target.getAttribute('role') === 'textbox';
}

export function useDoubleEsc(
  enabled: boolean,
  onDoubleEsc: () => void,
  maxIntervalMs = 500,
): { armed: boolean } {
  const [armed, setArmed] = useState(false);
  const lastPressRef = useRef(0);
  const callbackRef = useRef(onDoubleEsc);
  const disarmTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  callbackRef.current = onDoubleEsc;

  useEffect(() => {
    if (!enabled) {
      lastPressRef.current = 0;
      setArmed(false);
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.repeat || event.isComposing) {
        return;
      }
      if (isEditableTarget(event.target)) {
        return;
      }
      const now = performance.now();
      if (lastPressRef.current > 0 && now - lastPressRef.current <= maxIntervalMs) {
        lastPressRef.current = 0;
        setArmed(false);
        event.preventDefault();
        callbackRef.current();
        return;
      }
      lastPressRef.current = now;
      setArmed(true);
      if (disarmTimerRef.current !== null) {
        clearTimeout(disarmTimerRef.current);
      }
      disarmTimerRef.current = setTimeout(() => {
        lastPressRef.current = 0;
        setArmed(false);
        disarmTimerRef.current = null;
      }, maxIntervalMs);
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      if (disarmTimerRef.current !== null) {
        clearTimeout(disarmTimerRef.current);
        disarmTimerRef.current = null;
      }
    };
  }, [enabled, maxIntervalMs]);

  return { armed };
}
