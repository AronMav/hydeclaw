import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true,
  images: { unoptimized: true },
  experimental: {
    adapterPath: require.resolve("./build/adapter.cjs"),
  },
};

export default nextConfig;
