/** Default folder name for `source`: its last path segment without a trailing ".git", as git clone picks it. */
export function cloneFolderName(source: string) {
  const segments = source.trim().split(/[\\/:]/).filter(Boolean);
  // A path to the Git directory itself (repo/.git) is named after the folder that holds it.
  if (segments.length > 1 && /^\.git$/i.test(segments[segments.length - 1])) segments.pop();
  return (segments[segments.length - 1] ?? "").replace(/\.git$/i, "");
}

/** `name` inside `parent`, with the separator `parent` already uses; null when `name` is not a single folder name. */
export function cloneDestination(parent: string, name: string) {
  const base = parent.trim();
  const folder = name.trim();
  if (!base || !folder || folder === "." || folder === ".." || /[\\/]/.test(folder)) return null;
  const separator = base.includes("\\") ? "\\" : "/";
  return `${base.replace(/[\\/]+$/, "")}${separator}${folder}`;
}
