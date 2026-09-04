<script lang="ts">
  import type { Snippet } from "svelte";
  import AppShell from "$lib/components/AppShell.svelte";
  import "$lib/styles/app.css";
  interface Props { children: Snippet; }
  let { children }: Props = $props();
</script>
<AppShell>{@render children()}</AppShell>
