import { useMemo, useCallback } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  useNodesState,
  useEdgesState,
  type Node,
  type Edge,
  MarkerType,
  Position,
  ConnectionLineType,
} from "@xyflow/react";
import dagre from "dagre";
import type { PlanStep, PlanEdge } from "../types";
import { PlanDagNode } from "./plan-dag-node";

import "@xyflow/react/dist/style.css";

const nodeTypes = {
  planStep: PlanDagNode,
};

interface PlanDagProps {
  steps: PlanStep[];
  edges: PlanEdge[];
}

const NODE_WIDTH = 200;
const NODE_HEIGHT = 80;

function getLayoutedElements(
  steps: PlanStep[],
  planEdges: PlanEdge[]
): { nodes: Node[]; edges: Edge[] } {
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: "TB", nodesep: 50, ranksep: 80 });

  // Add nodes
  for (const step of steps) {
    g.setNode(step.id, { width: NODE_WIDTH, height: NODE_HEIGHT });
  }

  // Add edges
  for (const edge of planEdges) {
    g.setEdge(edge.from, edge.to);
  }

  // Run layout
  dagre.layout(g);

  // Convert to React Flow nodes
  const nodes: Node[] = steps.map((step) => {
    const nodeWithPosition = g.node(step.id);
    return {
      id: step.id,
      type: "planStep",
      position: {
        x: nodeWithPosition.x - NODE_WIDTH / 2,
        y: nodeWithPosition.y - NODE_HEIGHT / 2,
      },
      data: { step },
      sourcePosition: Position.Bottom,
      targetPosition: Position.Top,
    };
  });

  // Convert to React Flow edges
  // Note: Using hex colors because CSS variables don't work in SVG inline styles
  const edges: Edge[] = planEdges.map((edge, index) => ({
    id: `e-${edge.from}-${edge.to}-${index}`,
    source: edge.from,
    target: edge.to,
    sourceHandle: null,
    targetHandle: null,
    type: "smoothstep",
    label: edge.when ? formatCondition(edge.when) : undefined,
    labelStyle: { fill: "#737373", fontSize: 10 },
    labelBgStyle: { fill: "#fafaf9", fillOpacity: 0.95 },
    labelBgPadding: [4, 2] as [number, number],
    labelBgBorderRadius: 4,
    style: {
      stroke: edge.when ? "#f59e0b" : "#a8a29e",
      strokeWidth: edge.when ? 2 : 1.5,
    },
    markerEnd: {
      type: MarkerType.ArrowClosed,
      color: edge.when ? "#f59e0b" : "#a8a29e",
      width: 20,
      height: 20,
    },
    animated: edge.when != null,
  }));

  return { nodes, edges };
}

function formatCondition(when: unknown): string {
  if (typeof when === "string") {
    return when.length > 30 ? when.slice(0, 27) + "..." : when;
  }
  const str = JSON.stringify(when);
  return str.length > 30 ? str.slice(0, 27) + "..." : str;
}

export function PlanDag({ steps, edges: planEdges }: PlanDagProps) {
  const { nodes: initialNodes, edges: initialEdges } = useMemo(
    () => getLayoutedElements(steps, planEdges),
    [steps, planEdges]
  );

  const [nodes, , onNodesChange] = useNodesState(initialNodes);
  const [edges, , onEdgesChange] = useEdgesState(initialEdges);

  const onInit = useCallback(() => {
    // Center the view on initial load
  }, []);

  if (steps.length === 0) {
    return (
      <div className="h-125 flex items-center justify-center text-muted-foreground">
        No steps to display
      </div>
    );
  }

  return (
    <div className="h-125 w-full rounded-lg border border-border bg-background/50">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onInit={onInit}
        nodeTypes={nodeTypes}
        defaultEdgeOptions={{ type: "smoothstep" }}
        connectionLineType={ConnectionLineType.SmoothStep}
        fitView
        fitViewOptions={{ padding: 0.2 }}
        minZoom={0.3}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        className="[&_.react-flow__node]:bg-transparent!"
      >
        <Background color="hsl(var(--border))" gap={16} size={1} />
        <Controls
          className="[&_button]:bg-card [&_button]:border-border [&_button]:text-foreground [&_button:hover]:bg-muted"
          showInteractive={false}
        />
      </ReactFlow>
    </div>
  );
}
