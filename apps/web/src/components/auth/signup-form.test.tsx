import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { push, refresh, signupWithPassword, toastError } = vi.hoisted(() => ({
  push: vi.fn(),
  refresh: vi.fn(),
  signupWithPassword: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, refresh }),
}));

vi.mock("@/lib/auth/client", async () => {
  const actual = await vi.importActual<typeof import("@/lib/auth/client")>("@/lib/auth/client");
  return { ...actual, signupWithPassword };
});

vi.mock("sonner", () => ({
  toast: { error: toastError },
}));

import { SignupForm } from "@/components/auth/signup-form";

function fillValidForm() {
  fireEvent.change(screen.getByLabelText("First name"), { target: { value: "Ada" } });
  fireEvent.change(screen.getByLabelText("Last name"), { target: { value: "Lovelace" } });
  fireEvent.change(screen.getByLabelText("Email"), { target: { value: "new@example.com" } });
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "correct horse" } });
  fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "correct horse" } });
}

describe("SignupForm", () => {
  beforeEach(() => {
    push.mockClear();
    refresh.mockClear();
    signupWithPassword.mockReset();
    toastError.mockClear();
  });

  it("shows the password requirements checklist as neutral before any submit attempt, with no toast", () => {
    render(<SignupForm redirectTo="/" />);
    expect(screen.getByText("At least 8 characters")).toBeInTheDocument();
    expect(toastError).not.toHaveBeenCalled();
  });

  it("the password requirement turns from unmet to met live as the user types, before any submit attempt", () => {
    render(<SignupForm redirectTo="/" />);
    const password = screen.getByLabelText("Password") as HTMLInputElement;

    fireEvent.change(password, { target: { value: "short" } });
    expect(screen.getByText("At least 8 characters")).not.toHaveClass("text-destructive");

    fireEvent.change(password, { target: { value: "long enough" } });
    expect(screen.getByText("At least 8 characters")).toHaveClass("text-health-healthy");
  });

  it("toasts one combined message for missing name/email fields and never calls signupWithPassword", async () => {
    render(<SignupForm redirectTo="/" />);
    fireEvent.click(screen.getByRole("button", { name: "Sign up" }));

    await waitFor(() => expect(toastError).toHaveBeenCalledTimes(1));
    const [message] = toastError.mock.calls[0];
    expect(message).toBe("Please fix the highlighted fields.");
    expect(signupWithPassword).not.toHaveBeenCalled();
  });

  it("shows a live confirm-password mismatch after the field is touched, and a match confirmation once they agree", async () => {
    render(<SignupForm redirectTo="/" />);

    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "correct horse" } });
    fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "different" } });
    fireEvent.blur(screen.getByLabelText("Confirm password"));

    expect(await screen.findByText("Passwords do not match.")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "correct horse" } });

    expect(await screen.findByText("Passwords match")).toBeInTheDocument();
    expect(screen.queryByText("Passwords do not match.")).not.toBeInTheDocument();
  });

  it("blocks submit on a password mismatch even when name/email are all valid", async () => {
    render(<SignupForm redirectTo="/" />);
    fillValidForm();
    fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "a different password" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign up" }));

    expect(await screen.findByText("Passwords do not match.")).toBeInTheDocument();
    expect(signupWithPassword).not.toHaveBeenCalled();
  });

  it("submits trimmed first/last name plus credentials via signupWithPassword and redirects on success", async () => {
    signupWithPassword.mockResolvedValue({ id: 1, email: "new@example.com", first_name: "Ada", last_name: "Lovelace", tenant: "abc" });
    render(<SignupForm redirectTo="/somewhere" />);

    fireEvent.change(screen.getByLabelText("First name"), { target: { value: "  Ada  " } });
    fireEvent.change(screen.getByLabelText("Last name"), { target: { value: "  Lovelace  " } });
    fireEvent.change(screen.getByLabelText("Email"), { target: { value: "new@example.com" } });
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "correct horse" } });
    fireEvent.change(screen.getByLabelText("Confirm password"), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign up" }));

    await waitFor(() => expect(signupWithPassword).toHaveBeenCalledWith("Ada", "Lovelace", "new@example.com", "correct horse"));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/somewhere"));
    expect(toastError).not.toHaveBeenCalled();
  });

  it("disables the submit button while the request is pending", async () => {
    let resolveSignup!: (value: { id: number; email: string; first_name: string; last_name: string; tenant: string }) => void;
    signupWithPassword.mockReturnValue(new Promise((resolve) => (resolveSignup = resolve)));
    render(<SignupForm redirectTo="/" />);

    fillValidForm();
    fireEvent.click(screen.getByRole("button", { name: "Sign up" }));

    expect(await screen.findByRole("button", { name: "Signing up…" })).toBeDisabled();
    resolveSignup({ id: 1, email: "new@example.com", first_name: "Ada", last_name: "Lovelace", tenant: "abc" });
  });

  it("has independent show/hide toggles for password and confirm password", () => {
    render(<SignupForm redirectTo="/" />);

    const password = screen.getByLabelText("Password") as HTMLInputElement;
    const confirm = screen.getByLabelText("Confirm password") as HTMLInputElement;
    expect(password.type).toBe("password");
    expect(confirm.type).toBe("password");

    fireEvent.click(screen.getAllByRole("button", { name: "Show password" })[0]);
    expect(password.type).toBe("text");
    expect(confirm.type).toBe("password");
  });
});
