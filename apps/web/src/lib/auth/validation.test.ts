import { describe, expect, it } from "vitest";
import { PASSWORD_REQUIREMENTS, validateConfirmPassword, validateEmail, validateName, validateRequired } from "@/lib/auth/validation";

describe("validateEmail", () => {
  it("requires a value", () => {
    expect(validateEmail("")).toBe("Email is required.");
  });

  it("rejects a malformed address", () => {
    expect(validateEmail("not-an-email")).toBe("Enter a valid email address.");
  });

  it("accepts a well-formed address", () => {
    expect(validateEmail("dev@example.com")).toBeNull();
  });
});

describe("validateRequired", () => {
  it("flags an empty value with the given field label", () => {
    expect(validateRequired("", "Password")).toBe("Password is required.");
  });

  it("does not enforce any shape beyond presence", () => {
    expect(validateRequired("x", "Password")).toBeNull();
  });
});

describe("validateName", () => {
  it("flags an empty or whitespace-only name", () => {
    expect(validateName("", "First name")).toBe("First name is required.");
    expect(validateName("   ", "First name")).toBe("First name is required.");
  });

  it("accepts a real name", () => {
    expect(validateName("Ada", "First name")).toBeNull();
  });
});

describe("validateConfirmPassword", () => {
  it("requires a value", () => {
    expect(validateConfirmPassword("correct horse", "")).toBe("Please confirm your password.");
  });

  it("flags a mismatch", () => {
    expect(validateConfirmPassword("correct horse", "wrong")).toBe("Passwords do not match.");
  });

  it("accepts a match", () => {
    expect(validateConfirmPassword("correct horse", "correct horse")).toBeNull();
  });
});

describe("PASSWORD_REQUIREMENTS", () => {
  it("the length requirement is unmet under 8 characters and met at/over it", () => {
    const length = PASSWORD_REQUIREMENTS.find((r) => r.id === "length")!;
    expect(length.test("short")).toBe(false);
    expect(length.test("exactly8")).toBe(true);
  });
});
