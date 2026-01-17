import { useParams } from "react-router-dom";

export function WorkspacePage() {
  const { wsId } = useParams();

  return (
    <div className="page">
      <h1>Workspace</h1>
      <p>{wsId}</p>
      <div className="placeholder">
        Workspace browser placeholder. Tree, file viewer, annotations live
        here.
      </div>
    </div>
  );
}
