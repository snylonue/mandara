// Tiny inline SVG icon set (stroke follows currentColor).
// Replaces text arrows / emoji in the UI.
type IconProps = {
  size?: number;
  className?: string;
};

function base(size?: number) {
  return {
    width: size ?? 16,
    height: size ?? 16,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 2,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    "aria-hidden": true,
  };
}

/** Globe — public visibility. */
export function IconPublic({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <circle cx="12" cy="12" r="10" />
      <path d="M2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
    </svg>
  );
}

/** Eye-off — private visibility. */
export function IconPrivate({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M9.88 9.88a3 3 0 1 0 4.24 4.24" />
      <path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68" />
      <path d="M6.61 6.61A13.526 13.526 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61" />
      <line x1="2" x2="22" y1="2" y2="22" />
    </svg>
  );
}

/** Puzzle piece — plugin-provided content. */
export function IconPlugin({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M19.4 14.5 16 11l3.4-3.5a2 2 0 0 0 0-2.8l-.1-.1a2 2 0 0 0-2.8 0L13 8 9.5 4.6a2 2 0 0 0-2.8 0l-.1.1a2 2 0 0 0 0 2.8L10 11l-3.4 3.5a2 2 0 0 0 0 2.8l.1.1a2 2 0 0 0 2.8 0L13 14l3.5 3.4a2 2 0 0 0 2.8 0l.1-.1a2 2 0 0 0 0-2.8z" />
    </svg>
  );
}

/** Open book. */
export function IconBook({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M2 4h6a4 4 0 0 1 4 4v12a3 3 0 0 0-3-3H2z" />
      <path d="M22 4h-6a4 4 0 0 0-4 4v12a3 3 0 0 1 3-3h7z" />
    </svg>
  );
}

/** Upload arrow. */
export function IconUpload({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="17 8 12 3 7 8" />
      <line x1="12" x2="12" y1="3" y2="15" />
    </svg>
  );
}

/** Search magnifier. */
export function IconSearch({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <circle cx="11" cy="11" r="8" />
      <line x1="21" x2="16.65" y1="21" y2="16.65" />
    </svg>
  );
}

/** Chevron left — prev. */
export function IconChevronLeft({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <polyline points="15 18 9 12 15 6" />
    </svg>
  );
}

/** Chevron right — next. */
export function IconChevronRight({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <polyline points="9 18 15 12 9 6" />
    </svg>
  );
}

/** Chevron left with a wall — back. */
export function IconArrowLeft({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <line x1="19" x2="5" y1="12" y2="12" />
      <polyline points="12 19 5 12 12 5" />
    </svg>
  );
}

/** Plus — create. */
export function IconPlus({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <line x1="12" x2="12" y1="5" y2="19" />
      <line x1="5" x2="19" y1="12" y2="12" />
    </svg>
  );
}

/** Map pin — shared progress snapshot. */
export function IconPin({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M20 10c0 6-8 12-8 12s-8-6-8-12a8 8 0 0 1 16 0Z" />
      <circle cx="12" cy="10" r="3" />
    </svg>
  );
}

/** Moon — dark theme. */
export function IconMoon({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
    </svg>
  );
}

/** Sun — light theme. */
export function IconSun({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2m0 16v2M4.93 4.93l1.41 1.41m11.32 11.32 1.41 1.41M2 12h2m16 0h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
    </svg>
  );
}

/** Log-out. */
export function IconLogout({ size, className }: IconProps) {
  return (
    <svg {...base(size)} className={className}>
      <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
      <polyline points="16 17 21 12 16 7" />
      <line x1="21" x2="9" y1="12" y2="12" />
    </svg>
  );
}

/** Bookshelf logo mark (filled). */
export function LogoMark({ size = 22, className }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden
      className={className}
    >
      <path d="M3 3h5a3 3 0 0 1 3 3v14a2 2 0 0 0-2-2H3z" opacity="0.75" />
      <path d="M21 3h-5a3 3 0 0 0-3 3v14a2 2 0 0 1 2-2h6z" />
      <rect x="2" y="20" width="20" height="2" rx="1" opacity="0.6" />
    </svg>
  );
}
