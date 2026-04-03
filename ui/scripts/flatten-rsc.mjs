/**
 * Post-build script: flatten Next.js RSC segment directories into dot-separated files.
 *
 * Next.js 16 static export produces:
 *   config/__next.!SEG/config/__PAGE__.txt
 *
 * But the client-side router requests:
 *   config/__next.!SEG.config.__PAGE__.txt
 *
 * This script creates the dot-separated copies so any static server works.
 */

import { readdirSync, statSync, copyFileSync, existsSync } from "fs";
import { join, relative } from "path";

const OUT_DIR = new URL("../out", import.meta.url).pathname.replace(/^\/([A-Z]:)/, "$1");

function flattenDir(pageDir) {
  const entries = readdirSync(pageDir);
  for (const entry of entries) {
    const full = join(pageDir, entry);
    if (!statSync(full).isDirectory()) continue;
    if (!entry.startsWith("__next.")) continue;

    // This is a __next.* directory — flatten its contents
    flattenRecursive(pageDir, entry, full);
  }
}

function flattenRecursive(pageDir, prefix, dir) {
  const entries = readdirSync(dir);
  for (const entry of entries) {
    const full = join(dir, entry);
    const flatName = `${prefix}.${entry}`;
    if (statSync(full).isDirectory()) {
      flattenRecursive(pageDir, flatName, full);
    } else {
      const dest = join(pageDir, flatName);
      if (!existsSync(dest)) {
        copyFileSync(full, dest);
      }
    }
  }
}

function walkPages(dir) {
  const entries = readdirSync(dir);
  for (const entry of entries) {
    const full = join(dir, entry);
    if (!statSync(full).isDirectory()) continue;
    if (entry === "_next") continue;
    flattenDir(full);
    walkPages(full);
  }
}

flattenDir(OUT_DIR);
walkPages(OUT_DIR);
console.log("RSC segments flattened.");
