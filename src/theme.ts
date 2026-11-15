export type Theme = "system" | "light" | "dark";

export function getTheme(): Theme {
  const saved = localStorage.getItem("synapse.theme");
  return saved === "light" || saved === "dark" ? saved : "system";
}

export function applyTheme(theme: Theme = getTheme()) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}
