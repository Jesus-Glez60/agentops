import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { push, refresh, loginWithPassword, toastError } = vi.hoisted(() => ({
  push: vi.fn(),
  refresh: vi.fn(),
  loginWithPassword: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, refresh }),
}));

vi.mock("@/lib/auth/client", async () => {
  const actual = await vi.importActual<typeof import("@/lib/auth/client")>("@/lib/auth/client");
  return { ...actual, loginWithPassword };
});

vi.mock("sonner", () => ({
  toast: { error: toastError },
}));

import { LoginForm } from "@/components/auth/login-form";

describe("LoginForm", () => {
  beforeEach(() => {
    push.mockClear();
    refresh.mockClear();
    loginWithPassword.mockReset();
    toastError.mockClear();
  });

  it("toasts an email error and shows the password error inline, without calling loginWithPassword", async () => {
    render(<LoginForm redirectTo="/" />);
    fireEvent.click(screen.getByRole("button", { name: "Log in" }));

    await waitFor(() => expect(toastError).toHaveBeenCalledWith("Email is required."));
    expect(await screen.findByText("Password is required.")).toBeInTheDocument();
    expect(loginWithPassword).not.toHaveBeenCalled();
  });

  it("does not show any error before a submit attempt", () => {
    render(<LoginForm redirectTo="/" />);
    expect(screen.queryByText("Password is required.")).not.toBeInTheDocument();
    expect(toastError).not.toHaveBeenCalled();
  });

  it("submits valid credentials via loginWithPassword and redirects to redirectTo on success", async () => {
    loginWithPassword.mockResolvedValue({ id: 1, email: "dev@example.com", first_name: "Ada", last_name: "Lovelace", tenant: "abc" });
    render(<LoginForm redirectTo="/somewhere" />);

    fireEvent.change(screen.getByLabelText("Email"), { target: { value: "dev@example.com" } });
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Log in" }));

    await waitFor(() => expect(loginWithPassword).toHaveBeenCalledWith("dev@example.com", "correct horse"));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/somewhere"));
  });

  it("disables the submit button while the request is pending", async () => {
    let resolveLogin!: (value: { id: number; email: string; first_name: string; last_name: string; tenant: string }) => void;
    loginWithPassword.mockReturnValue(new Promise((resolve) => (resolveLogin = resolve)));
    render(<LoginForm redirectTo="/" />);

    fireEvent.change(screen.getByLabelText("Email"), { target: { value: "dev@example.com" } });
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Log in" }));

    expect(await screen.findByRole("button", { name: "Logging in…" })).toBeDisabled();
    resolveLogin({ id: 1, email: "dev@example.com", first_name: "Ada", last_name: "Lovelace", tenant: "abc" });
  });

  it("has a show/hide password toggle that reveals the typed password", async () => {
    render(<LoginForm redirectTo="/" />);

    const passwordInput = screen.getByLabelText("Password") as HTMLInputElement;
    fireEvent.change(passwordInput, { target: { value: "correct horse" } });
    expect(passwordInput.type).toBe("password");

    fireEvent.click(screen.getByRole("button", { name: "Show password" }));
    expect(passwordInput.type).toBe("text");

    fireEvent.click(screen.getByRole("button", { name: "Hide password" }));
    expect(passwordInput.type).toBe("password");
  });
});
