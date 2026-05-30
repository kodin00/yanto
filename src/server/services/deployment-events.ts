import { EventEmitter } from "node:events";

export type DeploymentLogEvent = {
  deploymentId: string;
  logs: string;
  status: string;
  done: boolean;
};

class DeploymentEventBus extends EventEmitter {
  emitLogUpdate(event: DeploymentLogEvent) {
    this.emit(`deployment:${event.deploymentId}`, event);
  }

  onLogUpdate(deploymentId: string, listener: (event: DeploymentLogEvent) => void) {
    this.on(`deployment:${deploymentId}`, listener);
    return () => {
      this.removeListener(`deployment:${deploymentId}`, listener);
    };
  }
}

export const deploymentEvents = new DeploymentEventBus();
deploymentEvents.setMaxListeners(100);
