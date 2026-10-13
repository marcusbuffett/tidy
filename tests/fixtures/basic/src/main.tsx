import React, { lazy } from "react";
import { createRoot } from "react-dom/client";
import { add, type Point } from "@/lib/math";
import * as colors from "./lib/colors";
import { Status } from "./lib/status";
import { Button } from "ui";
import { format } from "shared";
import "./styles.css";
import type { User } from "./types/user";
const Heavy = lazy(() => import("./lib/heavy"));
const page = await import("./lib/page");
const { fetchAll } = await import("./lib/api");
export function App(props: { user: User; p: Point }) {
  const s = Status.Active;
  page.render();
  fetchAll();
  return <Button label={format(String(add(1, 2)))} color={colors.red} status={s} />;
}
createRoot(document.body).render(<App user={{ name: "x" }} p={{ x: 1, y: 2 }} />);
export const unusedInEntry = 1;
