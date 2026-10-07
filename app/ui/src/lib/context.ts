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
