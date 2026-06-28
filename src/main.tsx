import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "@blocknote/core/fonts/inter.css";
import "@blocknote/mantine/style.css";
// Wide wordmark font (Saira variable — has a width axis 50–125%). Bundled by Vite → offline.
import "@fontsource-variable/saira/wdth.css";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
