/**
 * Next.js App Router configuration for the Gold Signal Bot dashboard.
 *
 * The dashboard itself lives in `src/app` (App Router). All `/api/*` calls are
 * proxied to the Rust Axum engine running on port 8080. Override the backend
 * origin with RUST_BACKEND_URL when running behind Docker/compose.
 *
 * @type {import('next').NextConfig}
 */
const rustBackendUrl = (process.env.RUST_BACKEND_URL || 'http://127.0.0.1:8080').replace(
  /\/+$/,
  ''
);

const nextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  async rewrites() {
    return [
      // Rust Axum exposes /health (without the /api prefix).
      {
        source: '/api/health',
        destination: `${rustBackendUrl}/health`,
      },
      // Frontend naming alias for the Rust /api/test-telegram endpoint.
      {
        source: '/api/telegram/test',
        destination: `${rustBackendUrl}/api/test-telegram`,
      },
      {
        source: '/api/:path*',
        destination: `${rustBackendUrl}/api/:path*`,
      },
      {
        source: '/health',
        destination: `${rustBackendUrl}/health`,
      },
    ];
  },
};

export default nextConfig;
