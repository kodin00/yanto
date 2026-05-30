import { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";

type Props = { children: ReactNode };
type State = { error: Error | null };

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("ErrorBoundary caught:", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <section className="panel" style={{ margin: "2rem", textAlign: "center" }}>
          <h2>Something went wrong</h2>
          <p className="muted" style={{ margin: "1rem 0" }}>{this.state.error.message}</p>
          <button
            className="button primary"
            type="button"
            onClick={() => this.setState({ error: null })}
          >
            <span>Try again</span>
          </button>
        </section>
      );
    }
    return this.props.children;
  }
}
