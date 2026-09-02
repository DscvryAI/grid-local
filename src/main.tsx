import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { OverlayScrollbars } from "overlayscrollbars";
import "overlayscrollbars/overlayscrollbars.css";
import { markColdStartBegin } from "./utils/coldStartTiming";

// The very first line this module runs, so the cold-start measurement
// covers the real startup path, not just however much of it happens to
// run before some later import.
markColdStartBegin();

// Fonts bundled locally (spec §24: "no remote fonts") -- these used to load
// from Google Fonts on every launch via <link> tags in index.html, which
// is a real, confirmed violation of the zero-outbound-traffic promise, not
// a false positive. Inter Tight replaced IBM Plex Sans as Grid Local's own
// deliberate visual identity -- same self-hosted @fontsource mechanism,
// never fetched remotely. JetBrains Mono (already the intended code/data
// font) is unchanged.
import "@fontsource/inter-tight/300.css";
import "@fontsource/inter-tight/400.css";
import "@fontsource/inter-tight/400-italic.css";
import "@fontsource/inter-tight/500.css";
import "@fontsource/inter-tight/600.css";
import "@fontsource/inter-tight/700.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/600.css";

import "./index.css";
import "./scrollbar.css";
import App from "./App.tsx";
import { ErrorBoundary } from "./components/ErrorBoundary.tsx";
import "./i18n";
import { PlatformProvider } from "./contexts/platform";
import { ThemeProvider } from "./contexts/theme/ThemeProvider.tsx";
import { ModalProvider } from "./contexts/modal/ModalProvider.tsx";
import { Toaster } from "sonner";

async function bootstrap(): Promise<void> {
  // Apply OverlayScrollbars globally to body
  OverlayScrollbars(document.body, {
    scrollbars: {
      theme: "os-theme-custom",
      autoHide: "leave",
      autoHideDelay: 400,
    },
  });

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <ErrorBoundary>
        <PlatformProvider>
          <ThemeProvider>
            <ModalProvider>
              <App />
              <Toaster />
            </ModalProvider>
          </ThemeProvider>
        </PlatformProvider>
      </ErrorBoundary>
    </StrictMode>
  );
}

void bootstrap();
