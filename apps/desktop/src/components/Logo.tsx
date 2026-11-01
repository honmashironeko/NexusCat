import { useApp } from '../state/appStore';

export function Logo({
  size = 28,
  className,
}: {
  size?: number;
  className?: string;
}) {
  const { resolvedTheme } = useApp();
  const asset =
    resolvedTheme === 'dark'
      ? 'logo-pixel-screen-v3-graphite-dark.svg'
      : 'logo-pixel-screen-v3-graphite-light.svg';
  return (
    <img
      className={className ? `brand-logo ${className}` : 'brand-logo'}
      src={`${import.meta.env.BASE_URL}${asset}`}
      width={size}
      height={size}
      alt="NexusCat"
      draggable={false}
    />
  );
}
