import type { ReactNode } from 'react';

export type IconName =
  | 'home'
  | 'overview'
  | 'devices'
  | 'layout'
  | 'bell'
  | 'settings'
  | 'updates'
  | 'sun'
  | 'moon'
  | 'system'
  | 'minimize'
  | 'maximize'
  | 'close'
  | 'expand'
  | 'collapse'
  | 'chevronDown'
  | 'chevronRight'
  | 'arrowLeft'
  | 'arrowRight'
  | 'arrowUp'
  | 'arrowDown'
  | 'check'
  | 'warning'
  | 'info'
  | 'error'
  | 'link'
  | 'unlink'
  | 'discovery'
  | 'stop'
  | 'pause'
  | 'folder'
  | 'shield'
  | 'save'
  | 'volume'
  | 'clipboard'
  | 'layers'
  | 'eyeOff'
  | 'globe'
  | 'download'
  | 'rollback'
  | 'file'
  | 'power'
  | 'monitor'
  | 'laptop'
  | 'spark'
  | 'hand'
  | 'lock'
  | 'refresh';

function renderIcon(name: IconName): ReactNode {
  switch (name) {
    case 'home':
      return (
        <>
          <path d="m3.5 10.5 8.5-7 8.5 7" />
          <path d="M5.5 9.5V20h13V9.5M9.5 20v-6h5v6" />
        </>
      );
    case 'overview':
      return (
        <>
          <rect x="3" y="3" width="7" height="7" rx="1.6" />
          <rect x="14" y="3" width="7" height="7" rx="1.6" />
          <rect x="3" y="14" width="7" height="7" rx="1.6" />
          <rect x="14" y="14" width="7" height="7" rx="1.6" />
        </>
      );
    case 'devices':
      return (
        <>
          <rect x="2.5" y="4" width="9.5" height="7" rx="1.6" />
          <path d="M5.2 14.5h4.1M7.25 11v3.5" />
          <rect x="13.5" y="8.5" width="8" height="6" rx="1.5" />
          <path d="M15.8 18h3.4M17.5 14.5V18" />
        </>
      );
    case 'layout':
      return (
        <>
          <rect x="3" y="3" width="8" height="8" rx="1.6" />
          <rect x="14" y="3" width="7" height="5" rx="1.6" />
          <rect x="14" y="10" width="7" height="11" rx="1.6" />
          <rect x="3" y="14" width="8" height="7" rx="1.6" />
        </>
      );
    case 'bell':
      return (
        <>
          <path d="M18 9a6 6 0 0 0-12 0c0 6-2.5 7-2.5 8.5h17C20.5 16 18 15 18 9Z" />
          <path d="M10 21h4" />
        </>
      );
    case 'settings':
      return (
        <>
          <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
          <circle cx="15" cy="7" r="2.2" />
          <circle cx="9" cy="17" r="2.2" />
        </>
      );
    case 'updates':
      return (
        <>
          <path d="M12 3v11" />
          <path d="m7.5 10 4.5 4.5L16.5 10" />
          <path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
        </>
      );
    case 'sun':
      return (
        <>
          <circle cx="12" cy="12" r="4" />
          <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
        </>
      );
    case 'moon':
      return <path d="M20 14.5A8.5 8.5 0 0 1 9.5 4 8.5 8.5 0 1 0 20 14.5Z" />;
    case 'system':
      return (
        <>
          <rect x="3" y="4" width="18" height="13" rx="2" />
          <path d="M8 21h8M12 17v4" />
        </>
      );
    case 'minimize':
      return <path d="M5 12h14" />;
    case 'maximize':
      return <rect x="5" y="5" width="14" height="14" rx="1.5" />;
    case 'close':
      return <path d="M6 6l12 12M18 6 6 18" />;
    case 'expand':
      return (
        <>
          <path d="M9 4H4v5M15 4h5v5M20 15v5h-5M9 20H4v-5" />
        </>
      );
    case 'collapse':
      return (
        <>
          <path d="M9 4v5H4M20 9h-5V4M15 20v-5h5M4 15h5v5" />
        </>
      );
    case 'chevronDown':
      return <path d="m6 9 6 6 6-6" />;
    case 'chevronRight':
      return <path d="m9 6 6 6-6 6" />;
    case 'arrowLeft':
      return <path d="M19 12H5M11 6l-6 6 6 6" />;
    case 'arrowRight':
      return <path d="M5 12h14M13 6l6 6-6 6" />;
    case 'arrowUp':
      return <path d="M12 19V5M6 11l6-6 6 6" />;
    case 'arrowDown':
      return <path d="M12 5v14M6 13l6 6 6-6" />;
    case 'check':
      return (
        <>
          <circle cx="12" cy="12" r="9" />
          <path d="m8 12 2.5 2.5L16 9" />
        </>
      );
    case 'warning':
      return (
        <>
          <path d="M12 4 2.8 20h18.4L12 4Z" />
          <path d="M12 9v5M12 17.5h.01" />
        </>
      );
    case 'info':
      return (
        <>
          <circle cx="12" cy="12" r="9" />
          <path d="M12 11v6M12 7.5h.01" />
        </>
      );
    case 'error':
      return (
        <>
          <circle cx="12" cy="12" r="9" />
          <path d="m9 9 6 6M15 9l-6 6" />
        </>
      );
    case 'link':
      return (
        <>
          <path d="M10 13a4 4 0 0 0 5.7 0l2.3-2.3a4 4 0 0 0-5.7-5.7L11 6.3" />
          <path d="M14 11a4 4 0 0 0-5.7 0L6 13.3a4 4 0 0 0 5.7 5.7l1.3-1.3" />
        </>
      );
    case 'unlink':
      return (
        <>
          <path d="M9 15 7 17a4 4 0 0 1-5.7-5.7l2-2" />
          <path d="m15 9 2-2a4 4 0 0 1 5.7 5.7l-2 2" />
          <path d="M8 4v3M4 8h3M18 20v-2M20 18h-2" />
        </>
      );
    case 'discovery':
      return (
        <>
          <circle cx="12" cy="12" r="2" />
          <path d="M8.5 8.5a5 5 0 0 0 0 7M15.5 8.5a5 5 0 0 1 0 7" />
          <path d="M5.5 5.5a9 9 0 0 0 0 13M18.5 5.5a9 9 0 0 1 0 13" />
        </>
      );
    case 'stop':
      return <rect x="6" y="6" width="12" height="12" rx="2" />;
    case 'pause':
      return (
        <>
          <path d="M9 5v14" />
          <path d="M15 5v14" />
        </>
      );
    case 'folder':
      return (
        <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" />
      );
    case 'shield':
      return (
        <>
          <path d="M12 3 4.5 6v5.3c0 4.5 3.2 8.2 7.5 9.7 4.3-1.5 7.5-5.2 7.5-9.7V6L12 3Z" />
          <path d="m8.7 12 2.2 2.2 4.4-4.4" />
        </>
      );
    case 'save':
      return (
        <>
          <path d="M5 3h11l3 3v15H5Z" />
          <path d="M8 3v6h8V3M8 14h8v7H8Z" />
        </>
      );
    case 'volume':
      return (
        <>
          <path d="M4 10v4h3l4 3.5v-11L7 10H4Z" />
          <path d="M15 9.5a4 4 0 0 1 0 5M17.5 7a7 7 0 0 1 0 10" />
        </>
      );
    case 'clipboard':
      return (
        <>
          <rect x="5" y="4" width="14" height="17" rx="2" />
          <path d="M9 4V3h6v1M8.5 9h7M8.5 13h7M8.5 17h4" />
        </>
      );
    case 'layers':
      return (
        <>
          <path d="m12 3 8 4.5-8 4.5-8-4.5L12 3Z" />
          <path d="m4 12 8 4.5 8-4.5M4 16.5 12 21l8-4.5" />
        </>
      );
    case 'eyeOff':
      return (
        <>
          <path d="M3 3l18 18" />
          <path d="M10.6 6.2A9.8 9.8 0 0 1 12 6c5.5 0 9 5.5 9 6a16 16 0 0 1-3 3.6M6.5 7.6A16 16 0 0 0 3 12c.4.7 3.8 6 9 6a9.2 9.2 0 0 0 3.5-.7" />
          <path d="M9.9 9.9a3 3 0 0 0 4.2 4.2" />
        </>
      );
    case 'globe':
      return (
        <>
          <circle cx="12" cy="12" r="9" />
          <path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18" />
        </>
      );
    case 'download':
      return (
        <>
          <path d="M12 3v10M8 9l4 4 4-4" />
          <path d="M5 17v2a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-2" />
        </>
      );
    case 'rollback':
      return (
        <>
          <path d="M4 9a8 8 0 1 1 1.3 8" />
          <path d="M4 4v5h5" />
        </>
      );
    case 'file':
      return (
        <>
          <path d="M6 3h8l4 4v14H6Z" />
          <path d="M14 3v5h5M9 13h6M9 17h4" />
        </>
      );
    case 'power':
      return (
        <>
          <path d="M12 3v9" />
          <path d="M6.5 6.5a8 8 0 1 0 11 0" />
        </>
      );
    case 'monitor':
      return (
        <>
          <rect x="3" y="4" width="18" height="12" rx="2" />
          <path d="M8 20h8M12 16v4" />
        </>
      );
    case 'laptop':
      return (
        <>
          <rect x="5" y="4" width="14" height="10" rx="1.6" />
          <path d="M3 18h18l-2-4H5l-2 4Z" />
        </>
      );
    case 'spark':
      return (
        <>
          <path d="m12 3 1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8L12 3Z" />
          <path d="m18.5 16 .8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8.8-2.2Z" />
        </>
      );
    case 'hand':
      return (
        <>
          <path d="M8 11V6a1.5 1.5 0 0 1 3 0v5-6a1.5 1.5 0 0 1 3 0v6-5a1.5 1.5 0 0 1 3 0v8a6 6 0 0 1-6 6h-1a5 5 0 0 1-3.8-1.7L4.6 15A1.5 1.5 0 0 1 7 13.2L8 15v-4Z" />
        </>
      );
    case 'lock':
      return (
        <>
          <rect x="5" y="10" width="14" height="11" rx="2" />
          <path d="M8 10V7a4 4 0 0 1 8 0v3M12 14v3" />
        </>
      );
    case 'refresh':
      return (
        <>
          <path d="M20 8V4l-2 2a8 8 0 1 0 1.5 9" />
          <path d="M20 4h-4" />
        </>
      );
    default:
      return <circle cx="12" cy="12" r="8" />;
  }
}

export function Icon({
  name,
  size = 16,
  className,
}: {
  name: IconName;
  size?: number;
  className?: string;
}) {
  return (
    <svg
      className={className ? `icon ${className}` : 'icon'}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {renderIcon(name)}
    </svg>
  );
}
