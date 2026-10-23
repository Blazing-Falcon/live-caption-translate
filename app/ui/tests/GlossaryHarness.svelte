<script lang="ts">
  import { provideSession } from "../src/lib/context";
  import Glossary from "../src/control/pages/Glossary.svelte";
  import type { Session } from "../src/lib/events";

  // Stands in for ControlApp: the page reads the session from context and the config from a prop.
  let { session }: { session: Session } = $props();
  // The session is fixed for the lifetime of the mount.
  // svelte-ignore state_referenced_locally
  provideSession(session);
  // svelte-ignore state_referenced_locally
  const config = session.config;
</script>

{#if $config}
  <Glossary config={$config} />
{/if}
