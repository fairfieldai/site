import type { NextConfig } from "next";

// The site is served as static files from S3 behind CloudFront. A CloudFront
// Function maps /about and /about/ to /about/index.html, which trailingSlash
// produces. Dynamic behavior belongs in the Rust API under /api.
const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true,
  images: { unoptimized: true },
};

export default nextConfig;
