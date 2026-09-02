import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * Spec §24: "no remote fonts," "no remote images," "no background API
 * calls." §25: normal use must produce zero outbound traffic.
 *
 * This is a regression test for a real, confirmed incident: `index.html`
 * preconnected to and loaded IBM Plex Sans/JetBrains Mono from Google
 * Fonts on every launch, before any user action. Fonts are now bundled
 * locally via @fontsource (see `src/main.tsx`) -- this test asserts the
 * remote `<link>` tags never come back, not just that today's build
 * happens to be clean. IBM Plex Sans was later replaced by Inter Tight as
 * Grid Local's own visual identity -- same self-hosted mechanism, so this
 * test's assertion only needed the package name updated.
 */
describe("network isolation: index.html", () => {
  const html = readFileSync(resolve(__dirname, "../../index.html"), "utf-8");

  it("has no remote stylesheet or preconnect links", () => {
    expect(html).not.toMatch(/<link[^>]+href=["']https?:\/\//i);
    expect(html).not.toContain("fonts.googleapis.com");
    expect(html).not.toContain("fonts.gstatic.com");
  });

  it("has no remote script or resource references at all", () => {
    // Every src/href in the shipped HTML must be relative, a Vite
    // %BASE_URL% placeholder, or a fragment -- never an http(s) origin.
    const urlAttrs = [...html.matchAll(/(?:src|href)=["']([^"']+)["']/gi)].map(
      (m) => m[1]
    );
    for (const url of urlAttrs) {
      expect(url).not.toMatch(/^https?:\/\//i);
    }
  });
});

describe("network isolation: fonts are bundled locally", () => {
  const mainTsx = readFileSync(resolve(__dirname, "../main.tsx"), "utf-8");

  it("imports the local @fontsource packages instead of a remote font URL", () => {
    expect(mainTsx).toMatch(/@fontsource\/inter-tight/);
    expect(mainTsx).toMatch(/@fontsource\/jetbrains-mono/);
  });
});

describe("network isolation: shipped CSP", () => {
  // Prior tests only asserted "a CSP with default-src exists," which
  // would pass just as easily on a much leakier config. This parses the
  // REAL shipped directive values and fails on any actual remote origin,
  // not just a missing/present key.
  const tauriConf = JSON.parse(
    readFileSync(resolve(__dirname, "../../src-tauri/tauri.conf.json"), "utf-8")
  );
  const csp: string = tauriConf.app.security.csp;
  const directives = Object.fromEntries(
    csp
      .split(";")
      .map((d) => d.trim())
      .filter(Boolean)
      .map((d) => {
        const [name, ...values] = d.split(/\s+/);
        return [name, values];
      })
  );

  it("has no http(s) origin in any directive", () => {
    for (const values of Object.values(directives)) {
      for (const value of values) {
        expect(value).not.toMatch(/^https?:/i);
      }
    }
  });

  it("restricts every present fetch-affecting directive to self/data/blob only", () => {
    const allowedSources = new Set(["'self'", "'none'", "'unsafe-inline'", "data:", "blob:"]);
    for (const directive of ["default-src", "script-src", "style-src", "img-src", "connect-src", "font-src"]) {
      const values = directives[directive];
      if (!values) continue;
      for (const value of values) {
        expect(allowedSources.has(value)).toBe(true);
      }
    }
  });

  it("sets frame-ancestors to none", () => {
    expect(directives["frame-ancestors"]).toEqual(["'none'"]);
  });
});
