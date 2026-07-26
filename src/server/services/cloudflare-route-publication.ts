export type RoutePublicationLifecycle = {
  publishDns: () => Promise<string | null>;
  publishIngress: () => Promise<void>;
  prepareRuntime: () => Promise<void>;
  cleanupPrevious: () => Promise<void>;
  markActive: (dnsRecordId: string | null) => Promise<void>;
  markError: (message: string, dnsRecordId: string | null) => Promise<void>;
};

/** Coordinates non-transactional systems while keeping the database route recoverable. */
export async function runRoutePublicationLifecycle(steps: RoutePublicationLifecycle): Promise<void> {
  let dnsRecordId: string | null = null;
  try {
    dnsRecordId = await steps.publishDns();
    await steps.publishIngress();
    await steps.prepareRuntime();
    await steps.cleanupPrevious();
    await steps.markActive(dnsRecordId);
  } catch (error) {
    const message = error instanceof Error ? error.message : "Cloudflare route publication failed.";
    await steps.markError(message, dnsRecordId).catch(() => undefined);
    throw error;
  }
}
