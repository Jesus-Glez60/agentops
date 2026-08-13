import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PasswordRequirementsList } from "@/components/auth/password-requirements";

describe("PasswordRequirementsList", () => {
  it("shows an unmet requirement as neutral (not error) before a submit attempt", () => {
    render(<PasswordRequirementsList password="" showUnmetAsError={false} />);
    expect(screen.getByText("At least 8 characters")).toHaveClass("text-ink-500");
  });

  it("shows an unmet requirement as an error once a submit attempt has been made", () => {
    render(<PasswordRequirementsList password="" showUnmetAsError={true} />);
    expect(screen.getByText("At least 8 characters")).toHaveClass("text-destructive");
  });

  it("shows a met requirement as healthy regardless of submit-attempt state", () => {
    render(<PasswordRequirementsList password="long enough" showUnmetAsError={true} />);
    expect(screen.getByText("At least 8 characters")).toHaveClass("text-health-healthy");
  });
});
