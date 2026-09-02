import { Component, type ErrorInfo, type ReactNode } from "react";
import { AlertTriangle } from "lucide-react";
import { withTranslation, type WithTranslation } from "react-i18next";
import { Card, CardContent } from "@/components/ui/card";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { recordDiagnosticsEvent } from "@/services/diagnosticsApi";

interface Props extends WithTranslation {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

// Scoped sibling to the global ErrorBoundary in main.tsx. A render error
// inside one session's view (e.g. malformed message data from a real
// provider file) used to take down the entire app via the single
// top-level boundary. Wrapping just SessionView, keyed by the selected
// session's file_path, contains the crash to an inline card and lets
// picking a different session recover without a full app reload.
class SessionErrorBoundaryComponent extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error("Uncaught error in SessionView:", error, errorInfo);
    void recordDiagnosticsEvent({ kind: "crashed" });
  }

  render() {
    const { t } = this.props;
    if (this.state.hasError) {
      return (
        <div className="h-full flex items-center justify-center p-6">
          <Card className="max-w-md w-full">
            <CardContent className="p-6">
              <Alert variant="destructive">
                <AlertTriangle className="h-4 w-4" />
                <AlertTitle>
                  {t("error.sessionUnavailable", {
                    defaultValue: "Couldn't display this session",
                  })}
                </AlertTitle>
                <AlertDescription>
                  {t("error.sessionUnavailableDescription", {
                    defaultValue:
                      "This session couldn't be rendered. Try selecting a different session.",
                  })}
                </AlertDescription>
              </Alert>
            </CardContent>
          </Card>
        </div>
      );
    }

    return this.props.children;
  }
}

export const SessionErrorBoundary = withTranslation("components")(
  SessionErrorBoundaryComponent
);
