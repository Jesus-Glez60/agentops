import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { LoginForm } from "@/components/auth/login-form";
import { SignupForm } from "@/components/auth/signup-form";

// Next.js 16 Server Component searchParams is Promise-wrapped -- must
// await it (day-one-bug-checklist.md #6). `from` is set by middleware.ts
// when it redirects a signed-out visitor here; only trust it as a
// same-origin relative path to avoid it being used as an open redirect.
function sanitizeRedirect(from: string | undefined): string {
  if (from && from.startsWith("/") && !from.startsWith("//")) return from;
  return "/";
}

export default async function LoginPage({ searchParams }: { searchParams: Promise<{ from?: string }> }) {
  const params = await searchParams;
  const redirectTo = sanitizeRedirect(params.from);

  return (
    <main className="flex min-h-screen items-center justify-center bg-canvas p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="text-page-title">AgentOps</CardTitle>
          <CardDescription>Sign in to your account, or create a new one.</CardDescription>
        </CardHeader>
        <CardContent>
          <Tabs defaultValue="login">
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
              <SignupForm redirectTo={redirectTo} />
            </TabsContent>
          </Tabs>
        </CardContent>
      </Card>
    </main>
  );
}
