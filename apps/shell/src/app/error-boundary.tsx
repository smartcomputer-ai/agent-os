import { isRouteErrorResponse, useRouteError } from "react-router-dom";

export function AppErrorBoundary() {
  const error = useRouteError();
  let title = "Unexpected error";
  let detail = "Something went wrong.";

  if (isRouteErrorResponse(error)) {
    title = `${error.status} ${error.statusText}`;
    detail = typeof error.data === "string" ? error.data : error.statusText;
  } else if (error instanceof Error) {
    detail = error.message;
  }

  return (
    <div className="page">
      <h1>{title}</h1>
      <div className="placeholder">{detail}</div>
    </div>
  );
}
