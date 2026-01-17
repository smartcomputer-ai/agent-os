import { NavLink, Outlet } from "react-router-dom";
import { useWorld } from "./world-provider";

export function ShellLayout() {
  const { world, status } = useWorld();
  const manifestLabel = world.manifestHash
    ? `manifest ${world.manifestHash.slice(0, 8)}`
    : null;
  const headLabel =
    world.journalHead != null ? `head ${world.journalHead}` : null;

  return (
    <div className="shell">
      <aside className="shell-sidebar">
        <div className="shell-brand">AgentOS</div>
        <div className="shell-status">
          {status.connected ? "Connected" : "Disconnected"}
        </div>
        <div className="shell-status">{status.label}</div>

        <nav className="shell-nav">
          <NavItem to="/explorer">Explorer</NavItem>
          <NavItem to="/workspaces">Workspaces</NavItem>
          <NavItem to="/governance">Governance</NavItem>
        </nav>
      </aside>

      <div className="shell-main">
        <header className="shell-header">
          <div>
            <span className="shell-title">{world.name ?? "World"}</span>
            {world.version ? (
              <span className="shell-subtitle">v{world.version}</span>
            ) : null}
          </div>
          <div className="shell-header-meta">
            {manifestLabel ? <Pill label={manifestLabel} /> : null}
            {headLabel ? <Pill label={headLabel} /> : null}
          </div>
        </header>
        <main className="shell-content">
          <Outlet />
        </main>
      </div>
    </div>
  );
}

function NavItem(props: { to: string; children: React.ReactNode }) {
  return (
    <NavLink
      to={props.to}
      className={({ isActive }) =>
        isActive ? "nav-item nav-item-active" : "nav-item"
      }
      end={props.to === "/explorer"}
    >
      {props.children}
    </NavLink>
  );
}

function Pill({ label }: { label: string }) {
  return <span className="pill">{label}</span>;
}
