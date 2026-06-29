import type express from "express";
import { currentUser } from "./auth.js";

export class HttpError extends Error {
  constructor(public readonly status: number, message: string) {
    super(message);
  }
}

export function asyncRoute(handler: (req: express.Request, res: express.Response) => Promise<void>) {
  return (req: express.Request, res: express.Response, next: express.NextFunction) => {
    handler(req, res).catch(next);
  };
}

export function routeParam(req: express.Request, name: string) {
  const value = req.params[name];
  return Array.isArray(value) ? value[0] : value;
}

export function actor(req: express.Request) {
  return currentUser(req)?.username ?? "admin";
}

export function startEventStream(res: express.Response) {
  res.writeHead(200, {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache, no-transform",
    Connection: "keep-alive",
    "X-Accel-Buffering": "no"
  });
}

export function sendStreamEvent(res: express.Response, payload: unknown) {
  res.write(`data: ${JSON.stringify(payload)}\n\n`);
}
