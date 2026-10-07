import { mount } from "svelte";
import { createSession } from "../lib/events";
import "../lib/tokens.css";
import OverlayApp from "./OverlayApp.svelte";

const session = createSession({ kind: "overlay" });
const target = document.getElementById("app");
if (!target) throw new Error("Missing #app mount point");

const app = mount(OverlayApp, { target, props: { session } });
void app;

window.addEventListener("pagehide", () => session.dispose(), { once: true });
