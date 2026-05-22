import YAML from "yaml";

type ComposeDocument = {
  services?: Record<string, unknown>;
};

const restartOverrideFile = ".yanto.restart.override.yml";

export function autoStartOverrideFile() {
  return restartOverrideFile;
}

export function buildAutoStartOverride(composeContent: string) {
  const parsed = YAML.parse(composeContent) as ComposeDocument | null;
  const services = parsed?.services;
  if (!services || typeof services !== "object" || Array.isArray(services)) {
    throw new Error("Compose file must include a services object before auto start can be enabled.");
  }

  const serviceNames = Object.keys(services);
  if (!serviceNames.length) {
    throw new Error("Compose file must include at least one service before auto start can be enabled.");
  }

  return YAML.stringify({
    services: Object.fromEntries(serviceNames.map((serviceName) => [serviceName, { restart: "unless-stopped" }]))
  });
}
