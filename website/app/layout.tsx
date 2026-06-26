import type { Metadata } from "next";
import { ThemeScript } from "@/components/theme-script/index";
import "./globals.css";

export const metadata: Metadata = {
  title: "Cadmus",
  description: "Alternative reading application for Kobo devices",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <ThemeScript />
      </head>
      <body className="flex min-h-screen flex-col bg-kumo-surface text-kumo-default antialiased">
        {children}
      </body>
    </html>
  );
}
