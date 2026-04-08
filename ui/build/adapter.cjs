"use strict";
// ── flatten-rsc-segments adapter ──────────────────────────────────────────────
//
// Next.js 16 static export produces RSC segment directories like:
//   out/config/__next.!SEG/config/__PAGE__.txt
//
// But the client-side router requests dot-separated flat paths like:
//   out/config/__next.!SEG.config.__PAGE__.txt
//
// This adapter runs inside `next build` via experimental.adapterPath and renames
// (not copies) all nested __next.* segment files to their flat-name equivalents,
// then removes the now-empty __next.* directories and any empty subdirectories
// left inside them after renaming.
//
// References: FRONT-01, FRONT-02 (Phase 28 audit)
// ─────────────────────────────────────────────────────────────────────────────

const fs = require("fs");
const path = require("path");

// ── Helpers ───────────────────────────────────────────────────────────────────

/**
 * Given a file's absolute path, check whether it contains a __next.* path
 * component followed by more components. If so, return the target absolute path
 * where everything from __next.* onward is collapsed into a dot-separated
 * filename placed in the parent directory.
 *
 * Example (Windows):
 *   "D:\ui\out\config\__next.!SEG\config\__PAGE__.txt"
 *   => "D:\ui\out\config\__next.!SEG.config.__PAGE__.txt"
 *
 * Returns null if the path contains no __next.* component that has children.
 */
function computeRenamedPath(absPath) {
  const components = absPath.split(path.sep);
  const idx = components.findIndex((x) => x.startsWith("__next."));
  if (idx >= 0 && idx < components.length - 1) {
    const result = components.slice(0, idx);
    result.push(components.slice(idx).join("."));
    return result.join(path.sep);
  }
  return null;
}

/**
 * Recursively walk a directory and collect all file (non-directory) paths.
 */
async function walkFiles(dir) {
  const results = [];
  let entries;
  try {
    entries = await fs.promises.readdir(dir, { withFileTypes: true });
  } catch {
    return results;
  }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      const nested = await walkFiles(full);
      results.push(...nested);
    } else {
      results.push(full);
    }
  }
  return results;
}

/**
 * Recursively collect all __next.* directory absolute paths under a root.
 * Does NOT recurse into __next.* dirs themselves — we collect the top-level
 * segment dir entry points.
 */
async function findSegmentDirs(dir) {
  const results = [];
  let entries;
  try {
    entries = await fs.promises.readdir(dir, { withFileTypes: true });
  } catch {
    return results;
  }
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const full = path.join(dir, entry.name);
    if (entry.name.startsWith("__next.")) {
      results.push(full);
      // Also recurse to find any nested __next.* inside (shouldn't happen, but defensive)
      const nested = await findSegmentDirs(full);
      results.push(...nested);
    } else {
      const nested = await findSegmentDirs(full);
      results.push(...nested);
    }
  }
  return results;
}

/**
 * Recursively remove a directory tree if it is entirely empty.
 * Returns the number of directories removed.
 */
async function removeEmptyTree(dir) {
  let removed = 0;
  let entries;
  try {
    entries = await fs.promises.readdir(dir, { withFileTypes: true });
  } catch {
    return removed;
  }

  for (const entry of entries) {
    if (entry.isDirectory()) {
      removed += await removeEmptyTree(path.join(dir, entry.name));
    }
  }

  // Try to remove this dir (will succeed only if now empty)
  try {
    await fs.promises.rmdir(dir);
    removed++;
  } catch {
    // Not empty or other error — leave it
  }
  return removed;
}

// ── Adapter ───────────────────────────────────────────────────────────────────

/** @type {import("next/dist/build/adapter/build-complete").NextAdapter} */
const adapter = {
  name: "flatten-rsc-segments",

  async onBuildComplete(ctx) {
    // For static export (output: "export"), Next.js writes files to projectDir/out.
    // ctx.distDir is ".next" (relative to projectDir) — NOT the export output dir.
    const projectDir = ctx.projectDir;
    const outDir = path.join(projectDir, "out");

    let renamedCount = 0;
    let primaryMatchCount = 0;

    // ── Primary path: use ctx.outputs.staticFiles ────────────────────────────
    // file.filePath is an ABSOLUTE path to the file in the exported output.
    if (
      ctx.outputs &&
      Array.isArray(ctx.outputs.staticFiles) &&
      ctx.outputs.staticFiles.length > 0
    ) {
      for (const file of ctx.outputs.staticFiles) {
        const absSrc = file.filePath; // already absolute
        const absDst = computeRenamedPath(absSrc);
        if (absDst !== null) {
          primaryMatchCount++;
          try {
            await fs.promises.rename(absSrc, absDst);
            renamedCount++;
          } catch (err) {
            console.warn(
              `flatten-rsc-segments: rename failed ${absSrc} -> ${absDst}: ${err.message}`
            );
          }
        }
      }
    }

    // ── Fallback path: walk outDir directly ──────────────────────────────────
    // Used when segment .txt files are not classified as STATIC_FILE by Next.js,
    // or when ctx.outputs is unavailable.
    if (primaryMatchCount === 0) {
      let outExists = false;
      try {
        await fs.promises.access(outDir);
        outExists = true;
      } catch {
        // outDir does not exist — nothing to flatten
      }

      if (outExists) {
        const allFiles = await walkFiles(outDir);
        for (const absSrc of allFiles) {
          const absDst = computeRenamedPath(absSrc);
          if (absDst !== null) {
            try {
              await fs.promises.rename(absSrc, absDst);
              renamedCount++;
            } catch (err) {
              console.warn(
                `flatten-rsc-segments: rename failed ${absSrc} -> ${absDst}: ${err.message}`
              );
            }
          }
        }
      }
    }

    // ── Cleanup: remove empty __next.* directory trees ───────────────────────
    // After renaming, each __next.* dir may contain empty subdirectories
    // (e.g. out/access/__next.!SEG/access/ — the "access" subdir is now empty).
    // We remove the entire __next.* subtree if it is empty after renames.
    let cleanedCount = 0;

    let outExists = false;
    try {
      await fs.promises.access(outDir);
      outExists = true;
    } catch {
      // skip
    }

    if (outExists) {
      const segDirs = await findSegmentDirs(outDir);
      // Sort by descending path length so deepest dirs are attempted first.
      // This allows parent segment dirs to become empty after child cleanup.
      segDirs.sort((a, b) => b.length - a.length);
      for (const dir of segDirs) {
        cleanedCount += await removeEmptyTree(dir);
      }
    }

    console.log(
      `flatten-rsc-segments: renamed ${renamedCount} files, cleaned ${cleanedCount} directories`
    );
  },
};

module.exports = adapter;
