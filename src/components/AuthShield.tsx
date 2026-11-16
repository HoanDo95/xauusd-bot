'use client';

import { useEffect, useState } from 'react';

/**
 * Opaque privacy shield for Basic-auth deployments.
 *
 * The browser's native Basic auth dialog sits on top of the current page. If the
 * HTML was cached before auth was enabled, the dashboard could remain visible
 * behind that dialog. This shield covers the viewport until an authenticated
 * API request succeeds; it is removed immediately when auth is already valid.
 */
export default function AuthShield() {
  const [visible, setVisible] = useState(true);
  const [showReload, setShowReload] = useState(false);

  useEffect(() => {
    let cancelled = false;

    const check = async () => {
      try {
        const response = await fetch('/api/health', {
          cache: 'no-store',
          credentials: 'same-origin',
        });
        if (!cancelled && response.ok) {
          setVisible(false);
        }
      } catch {
        // Keep the shield; the API may be unavailable.
      }
    };

    void check();
    const reloadTimer = window.setTimeout(() => {
      if (!cancelled) setShowReload(true);
    }, 4000);

    return () => {
      cancelled = true;
      window.clearTimeout(reloadTimer);
    };
  }, []);

  if (!visible) return null;

  return (
    <div
      className="fixed inset-0 z-[9999] flex flex-col items-center justify-center gap-3 bg-neutral-950 text-neutral-300"
      role="status"
      aria-live="polite"
    >
      <div className="text-3xl" aria-hidden="true">
        🔒
      </div>
      <div className="text-sm font-semibold">Đang xác thực dashboard...</div>
      <div className="max-w-xs text-center text-[11px] leading-relaxed text-neutral-500">
        Đăng nhập khi trình duyệt hiển thị hộp thoại. Nội dung dashboard được ẩn cho tới khi
        xác thực thành công.
      </div>
      {showReload && (
        <button
          type="button"
          onClick={() => window.location.reload()}
          className="mt-1 rounded-md border border-neutral-700 bg-neutral-900 px-3 py-1.5 text-[11px] font-semibold text-neutral-200 hover:border-neutral-500"
        >
          Tải lại trang
        </button>
      )}
    </div>
  );
}
