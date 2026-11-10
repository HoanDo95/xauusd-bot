/**
 * Next.js App Router configuration for the Gold Signal Bot dashboard.
 *
 * The dashboard lives in `src/app` (App Router). In development, `/api/*` and
 * `/health` are proxied to the Rust Axum engine (default port 8090). For the
 * production image, `NEXT_STATIC_EXPORT=1` emits a fully static `out/` bundle
 * that Nginx serves with its own same-origin `/api` proxy.
 *
 * @type {import('next').NextConfig}
 */
const rustBackendUrl = (process.env.RUST_BACKEND_URL || 'http://127.0.0.1:8090').replace(
  /\/+$/,
  ''
);

const isStaticExport = process.env.NEXT_STATIC_EXPORT === '1';

const nextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  output: isStaticExport ? 'export' : undefined,
  images: {
    unoptimized: true,
  },
  ...(isStaticExport
    ? {}
    : {
        async rewrites() {
          return [
            {
              source: '/api/:path*',
              destination: `${rustBackendUrl}/api/:path*`,
            },
            {
              source: '/health',
              destination: `${rustBackendUrl}/api/health`,
            },
          ];
        },
      }),
};

export default nextConfig;
