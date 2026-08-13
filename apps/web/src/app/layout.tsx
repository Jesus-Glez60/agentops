import type { Metadata } from "next";
import { JetBrains_Mono } from "next/font/google";
import "./globals.css";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";

// JetBrains Mono for both body copy and code -- one deliberate typeface
// choice for the whole app instead of a sans/mono split. Both --font-sans
// and --font-mono (globals.css) point at this single variable.
const jetbrainsMono = JetBrains_Mono({
  variable: "--font-body-sans",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "AgentOps",
  description: "Dev-intelligence dashboard: repo graphs, semantic search, and library docs.",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      className={`dark ${jetbrainsMono.variable} h-full antialiased`}
    >
      <body className="min-h-full flex flex-col">
        <TooltipProvider>{children}</TooltipProvider>
        <Toaster />
      </body>
    </html>
  );
}
