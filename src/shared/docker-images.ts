const imageComponent = "[a-z0-9]+(?:(?:[._]|__|[-]+)[a-z0-9]+)*";
const registry = "(?:localhost|[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?)(?::[0-9]+)?";
const tag = "[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}";
const digest = "[A-Za-z][A-Za-z0-9]*(?:[-_+.][A-Za-z][A-Za-z0-9]*)*:[A-Fa-f0-9]{32,}";
const dockerImagePattern = new RegExp(`^(?=.{1,500}$)(?:${registry}/)?${imageComponent}(?:/${imageComponent})*(?::${tag})?(?:@${digest})?$`);

/** Accept a bare image reference or the copy-paste-friendly `docker pull IMAGE` form. */
export function dockerImageFromInput(input: string | null | undefined) {
  const value = input?.trim() ?? "";
  if (!value) return "";
  const command = value.match(/^docker\s+(?:image\s+)?pull\s+([^\s]+)$/i);
  const image = command?.[1] ?? (value.startsWith("docker ") ? "" : value);
  return image && dockerImagePattern.test(image) ? image : null;
}

export function dockerPullCommand(image: string | null | undefined) {
  return image?.trim() ? `docker pull ${image.trim()}` : "";
}

export function dockerImageName(image: string | null | undefined) {
  const value = image?.trim().split("@")[0] ?? "";
  const lastComponent = value.split("/").at(-1) ?? "";
  return lastComponent.replace(/:[^:]+$/, "");
}
