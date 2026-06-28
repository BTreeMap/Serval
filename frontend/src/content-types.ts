/** Canonical MIME types offered as content-type suggestions. Serval stores
 *  text, so the list favors the formats people actually paste as snippets:
 *  prose, markup, config and data. Each entry is the canonical IANA form
 *  (charset on the `text/*` types). The value at delivery time still prefers a
 *  filename-extension guess; this only sets the fallback stored on the route,
 *  and free text is always allowed for anything not listed here. */
export const COMMON_CONTENT_TYPES = [
  "text/plain; charset=utf-8",
  "text/html; charset=utf-8",
  "text/markdown; charset=utf-8",
  "text/css; charset=utf-8",
  "text/javascript; charset=utf-8",
  "text/csv; charset=utf-8",
  "text/tab-separated-values; charset=utf-8",
  "text/xml; charset=utf-8",
  "application/json",
  "application/ld+json",
  "application/yaml",
  "application/toml",
  "application/xml",
  "application/rss+xml",
  "application/atom+xml",
  "image/svg+xml",
] as const;
