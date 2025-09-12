import type { Metadata } from "next";
import { Space_Grotesk, Doto } from "next/font/google";
import "./globals.css";
import { Providers } from "./components/providers";

const spaceGrotesk = Space_Grotesk({
  variable: "--font-sans",
  subsets: ["latin"],
  display: "swap",
});

const doto = Doto({
  variable: "--font-mono",
  weight: "400",
  preload: false,
});

export const metadata: Metadata = {
  title: "Convenant",
  description:
    "Private trading covenants — negotiate on MagicBlock Ephemeral Rollups, settle on-chain. SOL · USDC · NFT.",
  icons: {
    icon: "/icon.svg",
    shortcut: "/icon.svg",
    apple: "/icon.svg",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className={`${spaceGrotesk.variable} ${doto.variable} antialiased`}>
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
