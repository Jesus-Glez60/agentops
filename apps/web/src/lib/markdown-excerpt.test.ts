import { describe, expect, it } from "vitest";
import { excerptFromMarkdown } from "./markdown-excerpt";

describe("excerptFromMarkdown", () => {
  it("returns an empty string for null/undefined content", () => {
    expect(excerptFromMarkdown(null)).toBe("");
    expect(excerptFromMarkdown(undefined)).toBe("");
  });

  it("strips headings, list markers, and emphasis into plain text", () => {
    const md = "# Title\n\n## Context\n\n- one\n- two\n\n**bold** and _italic_ text.";
    expect(excerptFromMarkdown(md, 200)).toBe("Title Context one two bold and italic text.");
  });

  it("truncates long content with an ellipsis at maxLength", () => {
    const long = "a".repeat(300);
    const result = excerptFromMarkdown(long, 50);
    expect(result.length).toBe(51); // 50 chars + ellipsis
    expect(result.endsWith("…")).toBe(true);
  });

  it("does not truncate content already under maxLength", () => {
    expect(excerptFromMarkdown("short text", 160)).toBe("short text");
  });
});
