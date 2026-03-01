import { useState, useEffect, useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { Link } from "react-router-dom";
import {
  Library,
  FileType,
  Zap,
  Shield,
  Lock,
  Eye,
  EyeOff,
  ExternalLink,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { TreeNode, TreeLeaf } from "./tree-node";
import { isSysNamespace, type NamedRef } from "../../lib/manifest-types";

const SHOW_SYS_KEY = "aos-explorer-show-sys";

interface SupportingDefsSectionProps {
  schemas: NamedRef[];
  effects: NamedRef[];
  caps: NamedRef[];
  policies: NamedRef[];
}

export function SupportingDefsSection({
  schemas,
  effects,
  caps,
  policies,
}: SupportingDefsSectionProps) {
  const navigate = useNavigate();
  const [showSys, setShowSys] = useState(() => {
    if (typeof window !== "undefined") {
      return localStorage.getItem(SHOW_SYS_KEY) === "true";
    }
    return false;
  });

  // Persist preference
  useEffect(() => {
    localStorage.setItem(SHOW_SYS_KEY, String(showSys));
  }, [showSys]);

  // Split defs by namespace
  const { userSchemas, sysSchemas } = useMemo(() => {
    const user: NamedRef[] = [];
    const sys: NamedRef[] = [];
    for (const s of schemas) {
      if (isSysNamespace(s.name)) {
        sys.push(s);
      } else {
        user.push(s);
      }
    }
    return { userSchemas: user, sysSchemas: sys };
  }, [schemas]);

  const { userEffects, sysEffects } = useMemo(() => {
    const user: NamedRef[] = [];
    const sys: NamedRef[] = [];
    for (const e of effects) {
      if (isSysNamespace(e.name)) {
        sys.push(e);
      } else {
        user.push(e);
      }
    }
    return { userEffects: user, sysEffects: sys };
  }, [effects]);

  const { userCaps, sysCaps } = useMemo(() => {
    const user: NamedRef[] = [];
    const sys: NamedRef[] = [];
    for (const c of caps) {
      if (isSysNamespace(c.name)) {
        sys.push(c);
      } else {
        user.push(c);
      }
    }
    return { userCaps: user, sysCaps: sys };
  }, [caps]);

  const { userPolicies, sysPolicies } = useMemo(() => {
    const user: NamedRef[] = [];
    const sys: NamedRef[] = [];
    for (const p of policies) {
      if (isSysNamespace(p.name)) {
        sys.push(p);
      } else {
        user.push(p);
      }
    }
    return { userPolicies: user, sysPolicies: sys };
  }, [policies]);

  const totalSys =
    sysSchemas.length + sysEffects.length + sysCaps.length + sysPolicies.length;
  const totalUser =
    userSchemas.length + userEffects.length + userCaps.length + userPolicies.length;

  const handleDefClick = (kind: string, name: string) => {
    navigate(`/manifest/defs/${kind}/${encodeURIComponent(name)}`);
  };

  const totalDefs = schemas.length + effects.length + caps.length + policies.length;

  return (
    <>
      <CardHeader className="flex flex-row items-center justify-between space-y-0">
        <CardTitle className="text-lg flex items-center gap-2">
          <Library className="w-5 h-5 text-muted-foreground" />
          Supporting Defs
          <span className="text-sm font-normal text-muted-foreground">
            {totalUser}
            {totalSys > 0 && !showSys && ` + ${totalSys} sys/`}
          </span>
        </CardTitle>
        {/* Toggle for sys/ visibility */}
        {totalSys > 0 && (
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-xs"
            onClick={() => setShowSys(!showSys)}
          >
            {showSys ? (
              <EyeOff className="w-3 h-3 mr-1" />
            ) : (
              <Eye className="w-3 h-3 mr-1" />
            )}
            {showSys ? "Hide" : "Show"} sys/
          </Button>
        )}
      </CardHeader>
      <CardContent className="space-y-1">
        {/* Schemas */}
        <DefKindNode
        kind="defschema"
        label="Schemas"
        icon={<FileType className="w-3.5 h-3.5 text-blue-500" />}
        userDefs={userSchemas}
        sysDefs={sysSchemas}
        showSys={showSys}
        onDefClick={handleDefClick}
      />

      {/* Effects */}
      <DefKindNode
        kind="defeffect"
        label="Effects"
        icon={<Zap className="w-3.5 h-3.5 text-orange-500" />}
        userDefs={userEffects}
        sysDefs={sysEffects}
        showSys={showSys}
        onDefClick={handleDefClick}
      />

      {/* Caps */}
      <DefKindNode
        kind="defcap"
        label="Capabilities"
        icon={<Shield className="w-3.5 h-3.5 text-yellow-500" />}
        userDefs={userCaps}
        sysDefs={sysCaps}
        showSys={showSys}
        onDefClick={handleDefClick}
      />

      {/* Policies */}
      <DefKindNode
        kind="defpolicy"
        label="Policies"
        icon={<Lock className="w-3.5 h-3.5 text-red-500" />}
        userDefs={userPolicies}
        sysDefs={sysPolicies}
        showSys={showSys}
        onDefClick={handleDefClick}
      />

        {/* View all link */}
        <div className="flex items-center gap-2 py-2 mt-2">
          <Link
            to="/manifest/defs"
            className="text-xs text-muted-foreground hover:text-foreground flex items-center gap-1 transition-colors"
          >
            View all {totalDefs} definitions
            <ExternalLink className="w-3 h-3" />
          </Link>
        </div>
      </CardContent>
    </>
  );
}

interface DefKindNodeProps {
  kind: string;
  label: string;
  icon: React.ReactNode;
  userDefs: NamedRef[];
  sysDefs: NamedRef[];
  showSys: boolean;
  onDefClick: (kind: string, name: string) => void;
}

function DefKindNode({
  kind,
  label,
  icon,
  userDefs,
  sysDefs,
  showSys,
  onDefClick,
}: DefKindNodeProps) {
  const allDefs = showSys ? [...userDefs, ...sysDefs] : userDefs;
  const count = allDefs.length;
  const hiddenCount = showSys ? 0 : sysDefs.length;

  if (count === 0 && hiddenCount === 0) {
    return null;
  }

  return (
    <TreeNode
      label={label}
      icon={icon}
      badge={
        <span className="text-xs text-muted-foreground">
          {count}
          {hiddenCount > 0 && ` (+${hiddenCount})`}
        </span>
      }
      level={0}
    >
      {allDefs.map((def) => (
        <TreeLeaf
          key={def.name}
          level={1}
          label={
            <span
              className={isSysNamespace(def.name) ? "text-muted-foreground" : ""}
            >
              {def.name}
            </span>
          }
          onClick={() => onDefClick(kind, def.name)}
        />
      ))}
    </TreeNode>
  );
}
