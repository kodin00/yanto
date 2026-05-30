import { memo } from "react";
import type { DeploymentNode } from "../../shared/types";
import { dateTime } from "../app-utils";
import { StatusBadge } from "../components/ui";

type Props = {
  nodes: DeploymentNode[];
};

export const NodesView = memo(function NodesView(props: Props) {
  const { nodes } = props;

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>Deployment nodes</h2>
        <span className="count">{nodes.length} registered</span>
      </div>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Role</th>
              <th>Status</th>
              <th>Docker</th>
              <th>Projects</th>
              <th>Active</th>
              <th>Last seen</th>
            </tr>
          </thead>
          <tbody>
            {nodes.map((node) => (
              <tr key={node.id}>
                <td>{node.name}</td>
                <td>{node.role}</td>
                <td><StatusBadge status={node.status} /></td>
                <td>{node.dockerVersion ?? "-"}</td>
                <td>{node.projectCount ?? 0}</td>
                <td>{node.runningDeploymentCount ?? 0}</td>
                <td>{node.lastSeenAt ? dateTime(node.lastSeenAt) : "-"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {!nodes.length ? <p className="muted">No nodes registered yet.</p> : null}
    </section>
  );
});
