import { describe, expect, it } from "vitest";
import { importEdit } from "./indexCompletion";

const apply = (text: string, e: { at: number; insert: string } | null) =>
  e ? text.slice(0, e.at) + e.insert + text.slice(e.at) : text;

describe("importEdit", () => {
  it("adds a Kotlin import after the last one", () => {
    const text = "package a.b\n\nimport x.Y\n\nclass C : Init";
    expect(apply(text, importEdit(text, "a.c.Initializable", "kotlin"))).toBe(
      "package a.b\n\nimport x.Y\nimport a.c.Initializable\n\nclass C : Init",
    );
  });

  it("adds a Java import with a semicolon after the package line", () => {
    const text = "package a.b;\n\nclass C {}";
    expect(apply(text, importEdit(text, "a.c.Initializable", "java"))).toBe(
      "package a.b;\n\nimport a.c.Initializable;\n\nclass C {}",
    );
  });

  it("skips symbols of the same package, already imported ones and star imports", () => {
    expect(importEdit("package a.c\n\nclass C", "a.c.Initializable", "kotlin")).toBeNull();
    expect(importEdit("package a.b\nimport a.c.Initializable\n", "a.c.Initializable", "kotlin")).toBeNull();
    expect(importEdit("package a.b;\nimport a.c.*;\n", "a.c.Initializable", "java")).toBeNull();
  });

  it("does not take a longer name for an existing import", () => {
    const text = "package a.b\nimport a.c.InitializableFactory\n";
    expect(importEdit(text, "a.c.Initializable", "kotlin")).not.toBeNull();
  });
});
