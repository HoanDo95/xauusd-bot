'use client';

import { useEffect } from 'react';

export default function Error({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error(error);
  }, [error]);

  return (
    <div className="min-h-screen bg-neutral-950 text-neutral-100 flex items-center justify-center p-6">
      <div className="w-full max-w-lg rounded-2xl border border-rose-500/30 bg-neutral-900 p-6 flex flex-col gap-4">
        <div className="text-xs font-bold uppercase tracking-wider text-rose-300">
          UI error
        </div>
        <h1 className="text-lg font-bold text-white">Dashboard gặp lỗi giao diện</h1>
        <p className="text-sm text-neutral-400 leading-relaxed">
          Thường gặp sau khi deploy: browser vẫn giữ JS chunk cũ. Hãy tải lại trang.
          Nếu vẫn lỗi, mở DevTools → Console và gửi dòng lỗi đầu tiên.
        </p>
        <pre className="max-h-40 overflow-auto rounded-lg border border-neutral-800 bg-neutral-950 p-3 text-[11px] font-mono text-rose-300 whitespace-pre-wrap">
          {error.message || 'Unknown client error'}
          {error.digest ? `\nDigest: ${error.digest}` : ''}
        </pre>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            onClick={() => window.location.reload()}
            className="rounded-lg border border-sky-500/40 bg-sky-500/10 px-3 py-2 text-xs font-semibold text-sky-300 hover:border-sky-400"
          >
            Tải lại trang
          </button>
          <button
            type="button"
            onClick={reset}
            className="rounded-lg border border-neutral-700 bg-neutral-950 px-3 py-2 text-xs font-semibold text-neutral-300 hover:border-neutral-500"
          >
            Thử lại
          </button>
        </div>
      </div>
    </div>
  );
}
