import { useParams } from "react-router-dom";

export function PlanDiagramPage() {
  const { name } = useParams();

  return (
    <div className="page">
      <h1>Plan diagram</h1>
      <p>{name}</p>
      <div className="placeholder">
        Plan diagram placeholder. Canvas-based DAG viewer will live here.
      </div>
    </div>
  );
}
