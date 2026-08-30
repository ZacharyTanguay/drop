// ZOUGCLOUD(ZC-012): classify an error before showing a recovery page.
//
// Kept as a pure function, separate from error.vue, so it can be unit-tested
// and so the page stays free of branching logic.
//
// The classification deliberately prefers structured signals — the HTTP status
// code, and the app's own connection status — over matching on message text.
// Text is only consulted for the one case that carries no status code: the
// Tauri asset protocol, which reports a missing route as
// "asset not found: main/<route>" with nothing else to go on.

export enum ErrorKind {
  /** The route or asset does not exist. Retrying cannot help. */
  NotFound = "notFound",
  /** The session is gone or rejected; the auth flow should take over. */
  AuthInvalid = "authInvalid",
  /** Drop cannot reach the ZougCloud server. */
  ServerUnavailable = "serverUnavailable",
  /** Anything else, including transient failures worth retrying. */
  Unknown = "unknown",
}

/** The subset of a NuxtError this needs; keeps the function testable. */
export type ClassifiableError = {
  statusCode?: number;
  statusMessage?: string;
  message?: string;
  url?: string;
} | null | undefined;

/**
 * App connection status, when the caller has it. Optional so the error page
 * can classify without depending on app state — the very thing that may be
 * broken when it renders.
 */
export type ConnectionHint = "online" | "serverUnavailable" | undefined;

export function classifyError(
  error: ClassifiableError,
  connection: ConnectionHint = undefined,
): ErrorKind {
  if (!error) return ErrorKind.Unknown;

  const status = error.statusCode;

  // Structured signals first.
  if (status === 404) return ErrorKind.NotFound;
  if (status === 401 || status === 403) return ErrorKind.AuthInvalid;

  if (connection === "serverUnavailable") return ErrorKind.ServerUnavailable;

  // The Tauri asset protocol has no status code, so this one case has to be
  // recognised by its message. A missing Nuxt route becomes an asset request,
  // which is how "/id/me" surfaced as "asset not found: main/id/me".
  const text = `${error.statusMessage ?? ""} ${error.message ?? ""}`.toLowerCase();
  if (text.includes("asset not found")) return ErrorKind.NotFound;

  return ErrorKind.Unknown;
}
