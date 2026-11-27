import { describe, expect, it } from "vitest";
import { needsPasteConfirm } from "./paste";

describe("needsPasteConfirm", () => {
  it("una línea no pide confirmación", () => {
    expect(needsPasteConfirm("ls -la")).toBe(false);
    expect(needsPasteConfirm("")).toBe(false);
  });
  it("un salto final solo tampoco", () => {
    expect(needsPasteConfirm("ls\n")).toBe(false);
    expect(needsPasteConfirm("ls\r\n")).toBe(false);
  });
  it("varias líneas sí", () => {
    expect(needsPasteConfirm("a\nb")).toBe(true);
    expect(needsPasteConfirm("a\r\nb\r\n")).toBe(true);
    expect(needsPasteConfirm("\n\n")).toBe(true);
  });
});
