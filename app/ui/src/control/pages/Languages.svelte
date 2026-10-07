<script lang="ts">
  import { get } from "svelte/store";
  import { useSession } from "../../lib/context";
  import { reconcileChecked } from "../../lib/reconcile";
  import { isOtherLangTranslated, translateOtherPatch, type Config } from "../../lib/settings";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { config: liveConfig } = session;

  const OPTIONS = [
    { code: "ja", label: "Japanese" },
    { code: "ko", label: "Korean" },
  ] as const;

  async function toggle(event: Event, code: string): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save(`routing.${code}`, translateOtherPatch(config.routing.translate_other, code, input.checked));
    reconcileChecked(input, isOtherLangTranslated(get(liveConfig)?.routing.translate_other ?? [], code));
  }
</script>

<section class="page" aria-labelledby="languages-title">
  <h2 id="languages-title" class="section-title">Languages</h2>
  <dl class="kv">
    <dt>Speech language</dt>
    <dd>Chinese (Mandarin, Cantonese)</dd>
    <dt>Translate into</dt>
    <dd>English</dd>
    <dt>English speech</dt>
    <dd>Shown as heard</dd>
  </dl>
  <fieldset class="page" aria-label="Also translate">
    <legend class="field-label">Also translate</legend>
    <div class="stack">
      {#each OPTIONS as option (option.code)}
        <label class="check-row">
          <input
            type="checkbox"
            checked={isOtherLangTranslated(config.routing.translate_other, option.code)}
            onchange={(event) => toggle(event, option.code)}
          />
          <span>{option.label}</span>
        </label>
      {/each}
    </div>
    <p class="help">
      Off by default: misheard Chinese sometimes looks like these languages, and translating it gives confident nonsense.
    </p>
  </fieldset>
</section>

<style>
  .field-label {
    padding: 0;
    font-size: var(--text-label);
    color: var(--c-text-label);
  }
</style>
