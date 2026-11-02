import { mount } from "svelte";
import { createSession } from "../lib/events";
import { followTheme } from "../lib/theme";
import "../lib/fonts.css";
import "../lib/tokens.css";
import "../control/control.css";
import TrayApp from "./TrayApp.svelte";

const session = createSession({ kind: "control" });
followTheme(session);
const target = document.getElementById("app");
if (!target) throw new Error("Missing #app mount point");

mount(TrayApp, { target, props: { session } });

window.addEventListener("pagehide", () => session.dispose(), { once: true });
