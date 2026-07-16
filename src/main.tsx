import ReactDOM from "react-dom/client";
import App from "./App";

// No React.StrictMode: its dev-mode double-invoking of effects races with
// Tauri's JS-side event bookkeeping (`listen()`/unlisten() registered twice
// in quick succession), causing "listeners[eventId].handlerId" unhandled
// promise rejections in the login/session-output listeners.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<App />);
