import { mount } from "svelte";
import { createScreen } from "../lib/screen";
import "../lib/tokens.css";
import ScreenApp from "./ScreenApp.svelte";

const screen = createScreen();
const target = document.getElementById("app");
if (!target) throw new Error("Missing #app mount point");

mount(ScreenApp, { target, props: { screen } });

window.addEventListener("pagehide", () => screen.dispose(), { once: true });
