import type { Metadata } from 'next';
import type { ReactNode } from 'react';
import './globals.css';

const title = 'Gold Signal Bot — XAU/USD Sniper Engine';
const description =
  'XAU/USD Sniper engine with free live Binance PAXG data, ForexFactory news kill switch, EMA50 trend bias, dynamic S/R, Telegram alerts and an optional daily quota (unlimited by default).';

export const metadata: Metadata = {
  title,
  description,
  applicationName: 'Gold Signal Bot',
  keywords: ['XAU/USD', 'Gold', 'Sniper', 'Trading', 'Telegram', 'Binance PAXG'],
  openGraph: {
    title,
    description,
    type: 'website',
  },
  twitter: {
    card: 'summary_large_image',
    title,
    description,
  },
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="vi" className="dark">
      <body className="min-h-screen bg-neutral-950 text-neutral-100 font-sans antialiased selection:bg-amber-500/20 selection:text-amber-300">
        {children}
      </body>
    </html>
  );
}
