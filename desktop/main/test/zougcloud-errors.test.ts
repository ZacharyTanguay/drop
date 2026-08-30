// ZOUGCLOUD(ZC-012): the error page decides what to show — and crucially,
// whether to offer Retry — from these rules. Getting NotFound wrong is what
// produces the error loop the recovery page exists to prevent.

import { describe, expect, it } from "vitest";
import {
  classifyError,
  ErrorKind,
} from "../composables/zougcloud-errors";

describe("classifyError", () => {
  it("recognises a missing asset, which carries no status code", () => {
    // The real failure: /id/me had no route, so the Tauri asset protocol
    // reported it like this and the app went black.
    expect(
      classifyError({ message: "asset not found: main/id/me" }),
    ).toBe(ErrorKind.NotFound);
  });

  it("recognises a missing asset reported as a status message", () => {
    expect(
      classifyError({ statusMessage: "Asset not found: main/store/x" }),
    ).toBe(ErrorKind.NotFound);
  });

  it("prefers the status code over the message text", () => {
    // A 404 is authoritative even when the text says nothing useful.
    expect(classifyError({ statusCode: 404, message: "whatever" })).toBe(
      ErrorKind.NotFound,
    );
  });

  it("treats 401 and 403 as an auth problem, not a network one", () => {
    expect(classifyError({ statusCode: 401 })).toBe(ErrorKind.AuthInvalid);
    expect(classifyError({ statusCode: 403 })).toBe(ErrorKind.AuthInvalid);
  });

  it("uses the connection hint for an unreachable server", () => {
    expect(classifyError({ statusCode: 500 }, "serverUnavailable")).toBe(
      ErrorKind.ServerUnavailable,
    );
  });

  it("does not let the connection hint mask a missing route", () => {
    // Being offline does not make a nonexistent page retryable.
    expect(classifyError({ statusCode: 404 }, "serverUnavailable")).toBe(
      ErrorKind.NotFound,
    );
  });

  it("falls back to unknown, which is the retryable case", () => {
    expect(classifyError({ statusCode: 500 })).toBe(ErrorKind.Unknown);
    expect(classifyError({ message: "something odd" })).toBe(ErrorKind.Unknown);
  });

  it("tolerates a missing or empty error object", () => {
    expect(classifyError(null)).toBe(ErrorKind.Unknown);
    expect(classifyError(undefined)).toBe(ErrorKind.Unknown);
    expect(classifyError({})).toBe(ErrorKind.Unknown);
  });
});
