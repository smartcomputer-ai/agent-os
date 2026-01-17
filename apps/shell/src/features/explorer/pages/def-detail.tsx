import { useParams } from "react-router-dom";

export function DefDetailPage() {
  const { kind, name } = useParams();

  return (
    <div className="page">
      <h1>Def detail</h1>
      <p>
        {kind}/{name}
      </p>
      <div className="placeholder">
        Def detail placeholder. This will render schema-specific panels.
      </div>
    </div>
  );
}
