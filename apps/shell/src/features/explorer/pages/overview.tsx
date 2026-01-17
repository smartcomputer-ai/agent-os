import { Link } from "react-router-dom";

export function ExplorerOverview() {
  return (
    <div className="page">
      <h1>World Explorer</h1>
      <p>Read-only entry point for manifest, defs, and plans.</p>
      <div className="placeholder">
        <div className="section-title">Quick links</div>
        <ul className="link-list">
          <li>
            <Link to="/explorer/manifest">Manifest</Link>
          </li>
          <li>
            <Link to="/explorer/defs">Defs browser</Link>
          </li>
          <li>
            <Link to="/explorer/plans/example">Plan diagram (placeholder)</Link>
          </li>
        </ul>
      </div>
    </div>
  );
}
