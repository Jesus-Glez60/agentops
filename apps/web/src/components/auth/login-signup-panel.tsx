"use client";

import { useState } from "react";
import { AuthSplitLayout, GradientText } from "@/components/auth/auth-split-layout";
import { LoginForm } from "@/components/auth/login-form";
import { SignupForm } from "@/components/auth/signup-form";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

// Client wrapper so the left panel's eyebrow/headline can swap with the
// active tab (prototype: Login's "Welcome back." vs Signup's "Give your
// agents a map.") -- `LoginPage` itself stays a server component for the
// bootstrap-status fetch, this just owns the one piece of client state.
export function LoginSignupPanel({ hasAccounts, showSignupTab, defaultTab, redirectTo, inviteToken }: { hasAccounts: boolean; showSignupTab: boolean; defaultTab: "login" | "signup"; redirectTo: string; inviteToken?: string }) {
  const [activeTab, setActiveTab] = useState<"login" | "signup">(defaultTab);

  if (!showSignupTab) {
    return (
      <AuthSplitLayout eyebrow="Log in" eyebrowColor="var(--mauve)" headline={<>Welcome back.</>} subtitle="Sign in to your account to continue.">
        <AuthFormCard title="Welcome back">
          <LoginForm redirectTo={redirectTo} />
        </AuthFormCard>
      </AuthSplitLayout>
    );
  }

  const isSignup = activeTab === "signup";
  return (
    <AuthSplitLayout
      eyebrow={isSignup ? "Create account" : "Log in"}
      eyebrowColor="var(--mauve)"
      headline={
        isSignup ? (
          <>
            Give your agents <GradientText stops={["#cba6f7", "#f5c2e7", "#fab387"]}>a map.</GradientText>
          </>
        ) : (
          <>
            Welcome <GradientText stops={["#cba6f7", "#f5c2e7", "#fab387"]}>back.</GradientText>
          </>
        )
      }
      subtitle={!hasAccounts ? "Set up your AgentOps instance." : isSignup ? "Create an account to start mapping your repos." : "Sign in to your account, or create a new one."}
    >
      <AuthFormCard title={isSignup ? "Create your account" : "Log in"}>
        <Tabs value={activeTab} onValueChange={(v) => setActiveTab(v as "login" | "signup")}>
          <TabsList className="w-full">
            <TabsTrigger value="login" className="flex-1">
              Log in
            </TabsTrigger>
            <TabsTrigger value="signup" className="flex-1">
              Sign up
            </TabsTrigger>
          </TabsList>
          <TabsContent value="login" className="pt-4">
            <LoginForm redirectTo={redirectTo} />
          </TabsContent>
          <TabsContent value="signup" className="pt-4">
            <SignupForm redirectTo={redirectTo} inviteToken={inviteToken} />
          </TabsContent>
        </Tabs>
      </AuthFormCard>
    </AuthSplitLayout>
  );
}

function AuthFormCard({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-5 rounded-2xl border border-border-strong bg-panel p-6">
      <h2 className="text-display-card font-bold text-ink-100">{title}</h2>
      {children}
    </div>
  );
}
