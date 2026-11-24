'use client';

export default function GlobalError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  return (
    <html lang="vi">
      <body style={{ margin: 0, background: '#0a0a0a', color: '#e5e5e5', fontFamily: 'ui-sans-serif, system-ui' }}>
        <div style={{ minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center', padding: 24 }}>
          <div style={{ width: '100%', maxWidth: 560, border: '1px solid #7f1d1d', borderRadius: 16, padding: 24, background: '#171717' }}>
            <div style={{ fontSize: 11, letterSpacing: 1, textTransform: 'uppercase', color: '#fca5a5', fontWeight: 700 }}>
              UI error
            </div>
            <h1 style={{ fontSize: 18, margin: '12px 0 8px' }}>Dashboard gặp lỗi giao diện</h1>
            <p style={{ fontSize: 13, lineHeight: 1.6, color: '#a3a3a3' }}>
              Thường gặp sau khi deploy: browser vẫn giữ JS chunk cũ. Hãy tải lại trang.
            </p>
            <pre style={{ whiteSpace: 'pre-wrap', fontSize: 11, color: '#fca5a5', background: '#0a0a0a', border: '1px solid #262626', borderRadius: 8, padding: 12, maxHeight: 160, overflow: 'auto' }}>
              {error.message || 'Unknown client error'}
              {error.digest ? `\nDigest: ${error.digest}` : ''}
            </pre>
            <div style={{ display: 'flex', gap: 8, marginTop: 16 }}>
              <button
                type="button"
                onClick={() => window.location.reload()}
                style={{ border: '1px solid #0ea5e9', borderRadius: 8, background: '#082f49', color: '#7dd3fc', padding: '8px 12px', fontSize: 12, fontWeight: 600, cursor: 'pointer' }}
              >
                Tải lại trang
              </button>
              <button
                type="button"
                onClick={reset}
                style={{ border: '1px solid #404040', borderRadius: 8, background: '#0a0a0a', color: '#d4d4d4', padding: '8px 12px', fontSize: 12, fontWeight: 600, cursor: 'pointer' }}
              >
                Thử lại
              </button>
            </div>
          </div>
        </div>
      </body>
    </html>
  );
}
