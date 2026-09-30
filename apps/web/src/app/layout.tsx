import type { Metadata } from "next";
import { JetBrains_Mono, Schibsted_Grotesk } from "next/font/google";
import "./globals.css";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";

// Schibsted Grotesk for headings/prose, JetBrains Mono for code/labels --
// the Site v3 two-font system (see redesign plan). --font-sans/--font-heading
// (globals.css) point at the Grotesk variable; --font-mono points at this one.
const jetbrainsMono = JetBrains_Mono({
  variable: "--font-body-mono",
  subsets: ["latin"],
});

const schibstedGrotesk = Schibsted_Grotesk({
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
      className={`dark ${jetbrainsMono.variable} ${schibstedGrotesk.variable} h-full antialiased`}
    >
      <body className="min-h-full flex flex-col">
        <TooltipProvider>{children}</TooltipProvider>
        <Toaster />
      </body>
    </html>
  );
}
