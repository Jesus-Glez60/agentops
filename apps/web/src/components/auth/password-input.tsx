"use client";

import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";

// Shared by login/signup/confirm-password fields -- a show/hide toggle is
// standard password-field UX (masked by default, reveal on demand) so a
// user can verify what they typed rather than trusting the mask blindly.
// Built on shadcn's InputGroup primitives (same pattern the command
// palette's search input already uses) rather than a hand-rolled
// absolute-positioned button.
interface PasswordInputProps {
  id: string;
  value: string;
  onChange: (value: string) => void;
  onBlur?: () => void;
  autoComplete: "current-password" | "new-password";
  disabled?: boolean;
  ariaInvalid?: boolean;
}

export function PasswordInput({ id, value, onChange, onBlur, autoComplete, disabled, ariaInvalid }: PasswordInputProps) {
  const [visible, setVisible] = useState(false);

  return (
    <InputGroup>
      <InputGroupInput
        id={id}
        type={visible ? "text" : "password"}
        autoComplete={autoComplete}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
        disabled={disabled}
        aria-invalid={ariaInvalid}
      />
      <InputGroupAddon align="inline-end">
        <InputGroupButton type="button" size="icon-xs" onClick={() => setVisible((v) => !v)} disabled={disabled} aria-label={visible ? "Hide password" : "Show password"}>
          {visible ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
        </InputGroupButton>
      </InputGroupAddon>
    </InputGroup>
  );
}
