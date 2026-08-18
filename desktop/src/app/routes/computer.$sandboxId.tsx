import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/computer/$sandboxId")({
  component: TestComponent,
});

function TestComponent() {
  const { sandboxId } = Route.useParams();
  return <div>{sandboxId}</div>;
}
