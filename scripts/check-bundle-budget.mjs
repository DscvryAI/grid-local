#!/usr/bin/env node

/**
 * Bundle-size budget check.
 *
 * Informational only -- prints a report and warns when a chunk family
 * exceeds its budget, but always exits 0. This repo's own CI already
 * treats `cargo fmt --check` as informational for the same reason: no
 * real budget history exists yet to justify a hard gate, and a failing
 * build over a legitimate feature-driven size increase would just get
 * bypassed rather than respected. Revisit as a blocking check once
 * real budget history exists.
 *
 * Usage: node scripts/check-bundle-budget.mjs (after `pnpm build`)
 */

import { readdirSync, statSync } from "fs";
import { join, dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ASSETS_DIR = join(__dirname, "../dist/assets");

// Budgets are per "chunk family" (the manualChunks name, or the entry
// chunk itself), set generously above real measured sizes. `default`
// covers any chunk that doesn't match a named family (route/component-
// level code-split chunks, mostly).
const BUDGETS_KB = {
  index: 900, // main entry chunk
  "react-vendor": 700,
  markdown: 450,
  "ui-vendor": 300,
  default: 250,
};

// Strip Vite's content hash (`name-HASH.js`) to get the stable family
// name a budget can apply across builds. The hash itself never contains
// a hyphen, so excluding `-` from the trailing char class (unlike a
// naive `[A-Za-z0-9_-]+`) is what keeps a real multi-word chunk name
// like `react-vendor` or `i18n-en` intact instead of being swallowed
// into the hash strip.
function chunkFamily(filename) {
  const withoutExt = filename.replace(/\.(js|css)$/, "");
  const withoutHash = withoutExt.replace(/-[A-Za-z0-9_]{8,}$/, "");
  return withoutHash;
}

function formatKb(bytes) {
  return (bytes / 1024).toFixed(1);
}

function report(files, ext, label) {
  const matching = files.filter((f) => f.endsWith(`.${ext}`));
  const byFamily = new Map();
  for (const file of matching) {
    const size = statSync(join(ASSETS_DIR, file)).size;
    const family = chunkFamily(file);
    byFamily.set(family, (byFamily.get(family) ?? 0) + size);
  }

  const rows = [...byFamily.entries()].sort((a, b) => b[1] - a[1]);

  console.log(`\n📦 ${label} (dist/assets/*.${ext}, uncompressed):\n`);

  let overBudget = 0;
  for (const [family, bytes] of rows) {
    const budget = BUDGETS_KB[family] ?? BUDGETS_KB.default;
    const kb = formatKb(bytes);
    const status = bytes / 1024 > budget ? "⚠️  OVER" : "  ok";
    if (bytes / 1024 > budget) overBudget += 1;
    console.log(`${status}  ${family.padEnd(28)} ${kb.padStart(8)} kB  (budget ${budget} kB)`);
  }

  const totalKb = formatKb(rows.reduce((sum, [, bytes]) => sum + bytes, 0));
  console.log(`\nTotal: ${totalKb} kB across ${matching.length} ${ext.toUpperCase()} file(s).`);
  return overBudget;
}

function main() {
  let files;
  try {
    files = readdirSync(ASSETS_DIR);
  } catch {
    console.error(
      `❌ ${ASSETS_DIR} not found -- run \`pnpm build\` before this script.`
    );
    process.exitCode = 0; // informational: still don't fail the build
    return;
  }

  // JS and CSS reported separately, not merged into one family map --
  // `index-<hash>.js` and `index-<hash>.css` share the same stripped
  // family name ("index") but are different budget concerns.
  const overBudget =
    report(files, "js", "JavaScript bundle size report") +
    report(files, "css", "CSS bundle size report");

  if (overBudget > 0) {
    console.warn(
      `\n⚠️  ${overBudget} chunk family(ies) over budget -- informational only, not failing the build.`
    );
  }

  process.exitCode = 0;
}

main();
