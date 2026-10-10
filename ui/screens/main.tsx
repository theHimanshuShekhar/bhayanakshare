// The screen previews: `?view=<id>` draws the real app, with the stand-in shell of the UI tests
// and sample data, brought to one state; no `view` draws the index. Dev only: nothing here is
// reachable from ../index.html, so `vite build` never sees it.

import { screen } from "@testing-library/dom";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "../src/App";
import "../src/styles.css";
import { VIEWS, groups, shellFor, type View } from "./views";

const root = document.getElementById("root")!;
const id = new URLSearchParams(location.search).get("view");
const view = VIEWS.find((v) => v.id === id);

/** What a capture script waits for: `ready` once the view is in its state, `failed` if it could not be. */
const report = (state: "ready" | "failed", error = "") => {
  document.documentElement.dataset.screen = state;
  document.documentElement.dataset.error = error;
};

if (view) draw(view);
else drawIndex(id);

async function draw(view: View) {
  try {
    view.before?.();
    const device = shellFor(view);
    const rendered = createRoot(root);
    rendered.render(<App api={device.api} />);
    await screenReady();
    await view.setup?.(device);
    // Let what the last step did settle: a Home that waited, a sheet that took focus.
    await new Promise((resolve) => setTimeout(resolve, 200));
    report("ready");
  } catch (e) {
    console.error(e);
    report("failed", e instanceof Error ? e.message : String(e));
  }
}

/** Waits until the app is listening for events: My ID on Home, or the first-run screen. */
async function screenReady() {
  await screen.findByRole("heading", { name: /^(My ID|Welcome to BhayanakShare)$/ });
}

function drawIndex(unknown: string | null) {
  const rendered = createRoot(root);
  rendered.render(
    <StrictMode>
      <main>
        <h1>BhayanakShare screen previews</h1>
        <p>
          The real app with the UI tests&apos; stand-in shell and sample data, one URL per view. For
          development only: it is not part of the app. <code>pnpm --filter ui capture:screens</code>{" "}
          takes a picture of each into <code>ui/.screens-out/</code>.
        </p>
        {unknown !== null && <p role="alert">There is no view called {unknown}.</p>}
        {groups().map(([group, views]) => (
          <section key={group} aria-labelledby={`group-${group}`}>
            <h2 id={`group-${group}`}>{group}</h2>
            <ul>
              {views.map((v) => (
                <li key={v.id}>
                  <a href={`?view=${v.id}`} data-view={v.id} data-narrow={v.narrow ? "" : undefined} data-clip={v.clip}>
                    {v.title}
                  </a>{" "}
                  <code>{v.id}</code>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </main>
    </StrictMode>,
  );
}
