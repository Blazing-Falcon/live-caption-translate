import { getContext, setContext } from "svelte";
import type { Session } from "./events";

const SESSION_KEY = Symbol("session");

export function provideSession(session: Session): void {
  setContext(SESSION_KEY, session);
}

export function useSession(): Session {
  const session = getContext<Session | undefined>(SESSION_KEY);
  if (!session) throw new Error("No session in context");
  return session;
}

const PENDING_KEY = Symbol("pending");

export function providePending(pending: import("./pending").PendingChanges): void {
  setContext(PENDING_KEY, pending);
}

export function usePending(): import("./pending").PendingChanges {
  const pending = getContext<import("./pending").PendingChanges | undefined>(PENDING_KEY);
  if (!pending) throw new Error("No pending changes in context");
  return pending;
}
