// How much the app fits on screen: compact (the default) or
// comfortable. Only sizes change — theme.css keys every size off the
// `data-density` attribute this sets on <html>. Persisted per browser.
import { ref, watch } from "vue";

export const DENSITIES = ["compact", "comfortable"] as const;
export type Density = (typeof DENSITIES)[number];

const STORAGE_KEY = "datalib-density";

function stored(): Density {
  try {
    const s = localStorage.getItem(STORAGE_KEY);
    return DENSITIES.find((d) => d === s) ?? "compact";
  } catch {
    return "compact";
  }
}

export const density = ref<Density>(stored());

watch(
  density,
  (d) => {
    document.documentElement.dataset.density = d;
    try {
      localStorage.setItem(STORAGE_KEY, d);
    } catch {
      // Blocked storage: the choice lasts as long as the page.
    }
  },
  { immediate: true },
);
