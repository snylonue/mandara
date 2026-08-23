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
