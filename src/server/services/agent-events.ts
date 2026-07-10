import { EventEmitter } from "node:events";

export type AgentLiveEvent = {
  taskId: string;
  runId: string;
  sequence: number;
  kind: string;
  payload: Record<string, unknown>;
  done?: boolean;
};

class AgentEventBus extends EventEmitter {
  publish(event: AgentLiveEvent) {
    this.emit(`task:${event.taskId}`, event);
  }

  subscribe(taskId: string, listener: (event: AgentLiveEvent) => void) {
    this.on(`task:${taskId}`, listener);
    return () => this.removeListener(`task:${taskId}`, listener);
  }
}

export const agentEventBus = new AgentEventBus();
agentEventBus.setMaxListeners(100);
