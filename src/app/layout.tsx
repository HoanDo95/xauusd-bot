import type { ReactNode } from 'react';
import './globals.css';

// Next.js App Router metadata (free-standing object avoids a hard dependency on
// the `next` package while the Vite dev server remains the default runner).
export const metadata = {
  title: 'Gold Signal Bot — 1 Signal/Day Sniper Mode',
  description:
    'XAU/USD Sniper engine with free live Binance PAXG data, ForexFactory news kill switch, EMA50 trend bias, dynamic S/R, Telegram alerts and a hard 1-signal-per-day quota.',
  applicationName: 'Gold Signal Bot',
  keywords: ['XAU/USD', 'Gold', 'Sniper', 'Trading', 'Telegram', 'Binance PAXG'],
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
