import { isRouteErrorResponse, useRouteError } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

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
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-4 animate-in fade-in-0 slide-in-from-bottom-2">
      <Badge variant="destructive">Error</Badge>
      <Card className="bg-card/80">
        <CardHeader>
          <CardTitle className="text-2xl font-semibold">{title}</CardTitle>
          <CardDescription>Route error boundary.</CardDescription>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground">
          {detail}
        </CardContent>
      </Card>
    </div>
  );
}
