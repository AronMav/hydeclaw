/**
 * Sanitize a URL to prevent XSS via javascript:, vbscript:, or data:text/* protocols.
 * Allows: relative URLs (/path), http:, https:, and data:image/* (for inline images).
 * Returns "#" as a safe fallback for blocked or invalid URLs.
 */
export function sanitizeUrl(url: string): string {
  const trimmed = url.trim();
  if (!trimmed) return "#";

  // Relative URLs starting with "/" are safe internal paths (e.g. /uploads/*)
  if (trimmed.startsWith("/")) return trimmed;

  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return "#";
  }

  const protocol = parsed.protocol.toLowerCase();

  if (protocol === "http:" || protocol === "https:") return trimmed;

  // Allow data: URLs only for image/* MIME types
  if (protocol === "data:") {
    if (/^data:image\//i.test(trimmed)) return trimmed;
    return "#";
  }

  return "#";
}
