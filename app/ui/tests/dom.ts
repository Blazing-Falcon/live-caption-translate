import { flushSync, mount, unmount, type Component } from "svelte";

const mounted: { target: HTMLElement; instance: Record<string, unknown> }[] = [];

export const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

/** Lets pending promises and Svelte effects run. */
export async function settle(rounds = 4): Promise<void> {
  for (let index = 0; index < rounds; index += 1) {
    await flush();
    flushSync();
  }
}

// Svelte constrains component props to Record<string, any>; interface props only satisfy that form.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function render<Props extends Record<string, any>>(component: Component<Props>, props: NoInfer<Props>): HTMLElement {
  const target = document.createElement("div");
  document.body.append(target);
  const instance = mount(component, { target, props });
  mounted.push({ target, instance });
  flushSync();
  return target;
}

export function cleanup(): void {
  for (const { target, instance } of mounted.splice(0)) {
    void unmount(instance);
    target.remove();
  }
}

export function text(root: ParentNode): string {
  return (root.textContent ?? "").replace(/\s+/g, " ").trim();
}

export function q<T extends Element = HTMLElement>(root: ParentNode, selector: string): T {
  const found = root.querySelector<T>(selector);
  if (!found) throw new Error(`no element matches ${selector}`);
  return found;
}

export function byRole(root: ParentNode, role: string, name: string | RegExp): HTMLElement {
  const candidates = [...root.querySelectorAll<HTMLElement>(`[role="${role}"], ${implicitSelector(role)}`)];
  const match = candidates.find((element) => matches(accessibleName(element), name));
  if (!match) throw new Error(`no ${role} named ${String(name)}; have: ${candidates.map(accessibleName).join(" | ")}`);
  return match;
}

function implicitSelector(role: string): string {
  switch (role) {
    case "button":
      return "button";
    case "radio":
      return 'input[type="radio"]';
    case "checkbox":
      return 'input[type="checkbox"]';
    case "combobox":
      return "select";
    case "slider":
      return 'input[type="range"]';
    case "textbox":
      return 'input[type="text"]';
    default:
      return `[data-no-implicit-${role}]`;
  }
}

function matches(value: string, name: string | RegExp): boolean {
  return typeof name === "string" ? value === name : name.test(value);
}

export function accessibleName(element: HTMLElement): string {
  const label = element.getAttribute("aria-label");
  if (label) return label;
  if (element.id) {
    const forLabel = element.ownerDocument.querySelector(`label[for="${element.id}"]`);
    if (forLabel) return text(forLabel);
  }
  const wrapping = element.closest("label");
  if (wrapping) {
    const clone = wrapping.cloneNode(true) as HTMLElement;
    clone.querySelectorAll("select, input, textarea").forEach((control) => control.remove());
    return text(clone);
  }
  return text(element);
}

export function setValue(element: HTMLInputElement | HTMLSelectElement, value: string, events: string[] = ["input", "change"]): void {
  element.value = value;
  for (const name of events) element.dispatchEvent(new Event(name, { bubbles: true }));
}
