import { Suspense, lazy } from "react";
import {
  FolderSelectorContainer,
  GlobalSearchModalContainer,
} from "@/components/modals";

// Feedback and the Session Picker are occasional
// actions, not primary entry points -- code-split so their chunks only
// load the first time a user actually opens either. FolderSelector
// (settings/onboarding) and GlobalSearch (Cmd/Ctrl+K, a primary entry
// point) stay eager. Both containers render `null` when closed, so a
// `null` Suspense fallback is imperceptible even mid-load.
const FeedbackModalContainer = lazy(() =>
  import("@/components/modals/feedback/FeedbackModalContainer").then(
    (m) => ({ default: m.FeedbackModalContainer })
  )
);
const SessionPickerModalContainer = lazy(() =>
  import("@/components/modals/sessionPicker/SessionPickerModalContainer").then(
    (m) => ({ default: m.SessionPickerModalContainer })
  )
);

export const ModalContainer = () => {
  return (
    <>
      <FolderSelectorContainer />
      <Suspense fallback={null}>
        <FeedbackModalContainer />
      </Suspense>
      <GlobalSearchModalContainer />
      <Suspense fallback={null}>
        <SessionPickerModalContainer />
      </Suspense>
    </>
  );
};
