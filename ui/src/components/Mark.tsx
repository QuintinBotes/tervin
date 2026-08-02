/**
 * The Tervin mark.
 *
 * Inline SVG rather than an image file so it inherits the current colour and
 * stays crisp at any size. The geometry is the authoritative asset from the
 * brand system, unmodified: two tapered halves facing a narrow teal seam.
 */
export function Mark({ size = 20, plate = false }: { size?: number; plate?: boolean }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 128 128"
      role="img"
      aria-label="Tervin"
      style={{ flex: "none", display: "block" }}
    >
      {/* The plate is only for the application icon; in-app the mark sits on the
          surface directly, with clear space rather than a container. */}
      {plate && <rect width="128" height="128" rx="24" fill="var(--tervin-bg)" />}
      <path d="M39 22h20v84H39l11-42z" fill="var(--tervin-ink)" />
      <path d="M89 22H69v84h20L78 64z" fill="var(--tervin-muted)" />
      <path d="M61 22h6v84h-6z" fill="var(--tervin-accent)" />
    </svg>
  );
}
