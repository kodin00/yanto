import type {
  CloudflareRouteDiagnosticDnsRecord,
  CloudflareRouteDnsStatus,
  CloudflareRouteReachabilityStatus,
  CloudflareRouteTunnelStatus
} from "../../shared/types.js";

type DiagnosticInput = {
  routeEnabled: boolean;
  hostname: string;
  expectedDnsTarget: string | null;
  dnsRecords: CloudflareRouteDiagnosticDnsRecord[] | null;
  tunnelExists: boolean;
  tunnelRuntimeRunning: boolean | null;
  tunnelHealthy: boolean | null;
  reachabilityStatus: CloudflareRouteReachabilityStatus;
};

export type DiagnosticClassification = {
  dnsStatus: CloudflareRouteDnsStatus;
  tunnelStatus: CloudflareRouteTunnelStatus;
  reachabilityStatus: CloudflareRouteReachabilityStatus;
  messages: string[];
  recommendedFixes: string[];
};

function sameHostname(record: CloudflareRouteDiagnosticDnsRecord, hostname: string) {
  return record.name.toLowerCase() === hostname.toLowerCase();
}

function expectedCname(record: CloudflareRouteDiagnosticDnsRecord, expectedDnsTarget: string | null) {
  return Boolean(expectedDnsTarget) && record.type.toUpperCase() === "CNAME" && record.content.toLowerCase() === expectedDnsTarget?.toLowerCase();
}

function classifyDns(input: DiagnosticInput): CloudflareRouteDnsStatus {
  if (!input.expectedDnsTarget || !input.dnsRecords) return "unknown";
  const records = input.dnsRecords.filter((record) => sameHostname(record, input.hostname));
  if (!records.length) return "missing";
  const matchingCname = records.find((record) => expectedCname(record, input.expectedDnsTarget));
  const nonMatchingRecords = records.filter((record) => !expectedCname(record, input.expectedDnsTarget));
  if (matchingCname && !nonMatchingRecords.length) return "ok";
  if (matchingCname) return "conflict";
  if (records.some((record) => record.type.toUpperCase() === "CNAME")) return "mismatch";
  return "conflict";
}

function classifyTunnel(input: DiagnosticInput): CloudflareRouteTunnelStatus {
  if (!input.tunnelExists) return "missing";
  if (input.tunnelRuntimeRunning === null || input.tunnelHealthy === null) return "unknown";
  if (!input.tunnelRuntimeRunning) return "stopped";
  if (!input.tunnelHealthy) return "unhealthy";
  return "running";
}

export function classifyRouteDiagnostic(input: DiagnosticInput): DiagnosticClassification {
  const dnsStatus = classifyDns(input);
  const tunnelStatus = classifyTunnel(input);
  const messages: string[] = [];
  const recommendedFixes: string[] = [];

  if (!input.routeEnabled) {
    messages.push("Route is disabled.");
    recommendedFixes.push("Enable the Cloudflare route.");
  }

  if (dnsStatus === "missing") {
    messages.push("DNS record is missing.");
    recommendedFixes.push(`Create a CNAME for ${input.hostname} pointing to ${input.expectedDnsTarget}.`);
  } else if (dnsStatus === "mismatch") {
    messages.push("DNS CNAME points to a different target.");
    recommendedFixes.push(`Update the CNAME target to ${input.expectedDnsTarget}.`);
  } else if (dnsStatus === "conflict") {
    messages.push("Another DNS record uses this hostname.");
    recommendedFixes.push(`Replace conflicting same-name records with a CNAME to ${input.expectedDnsTarget}.`);
  } else if (dnsStatus === "unknown") {
    messages.push("DNS status could not be checked.");
    recommendedFixes.push("Validate Cloudflare settings and try again.");
  }

  if (tunnelStatus === "missing") {
    messages.push("Cloudflare tunnel is missing.");
    recommendedFixes.push("Republish the project hostname.");
  } else if (tunnelStatus === "stopped") {
    messages.push("Cloudflared connector is stopped.");
    recommendedFixes.push("Start or restart cloudflared for the route node.");
  } else if (tunnelStatus === "unhealthy") {
    messages.push("Cloudflare reports the tunnel as unhealthy.");
    recommendedFixes.push("Restart cloudflared and check Cloudflare tunnel health.");
  } else if (tunnelStatus === "unknown") {
    messages.push("Tunnel status could not be checked.");
    recommendedFixes.push("Check local Docker and Cloudflare API access.");
  }

  if (input.reachabilityStatus === "failed") {
    messages.push("Hostname is not reachable over HTTPS.");
    recommendedFixes.push("Check the project container, service target, and Cloudflare route.");
  } else if (input.reachabilityStatus === "unknown") {
    messages.push("Hostname reachability could not be checked.");
  }

  if (!messages.length) {
    messages.push("Hostname wiring looks healthy.");
  }

  return {
    dnsStatus,
    tunnelStatus,
    reachabilityStatus: input.reachabilityStatus,
    messages,
    recommendedFixes
  };
}
