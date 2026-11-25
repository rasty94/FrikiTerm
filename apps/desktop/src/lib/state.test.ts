import { expect, test } from "vitest";
import type { AppEvent } from "./types";

// Forma de los eventos que manda Rust (camelCase, etiqueta `type`).
test("AppEvent se discrimina por type", () => {
  const raw = '{"type":"connClosed","conn":3,"reason":null}';
  const ev = JSON.parse(raw) as AppEvent;
  expect(ev.type === "connClosed" && ev.conn).toBe(3);
});
