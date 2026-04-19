from __future__ import annotations

import json
import os
import shutil
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, Optional

from .types import EffectModeName, PathLike

_SDK_PATH_SENTINEL = "../../../../aos-wasm-sdk"

if TYPE_CHECKING:
    from ._core import WorldHarness


def _coerce_path(value: PathLike) -> Path:
    return Path(value).expanduser().resolve()


def _copy_authored_tree(source_root: Path, destination_root: Path) -> None:
    shutil.copytree(
        source_root,
        destination_root,
        dirs_exist_ok=True,
        ignore=shutil.ignore_patterns(".aos", ".git", "target", "__pycache__"),
    )


def _patch_workflow_sdk_path(workflow_dir: Path, sdk_root: Optional[Path]) -> None:
    if sdk_root is None:
        return
    cargo_toml = workflow_dir / "Cargo.toml"
    if not cargo_toml.exists():
        return
    cargo_text = cargo_toml.read_text()
    if _SDK_PATH_SENTINEL not in cargo_text:
        return
    cargo_toml.write_text(cargo_text.replace(_SDK_PATH_SENTINEL, str(sdk_root)))


def _write_minimal_sync_config(
    world_root: Path,
    *,
    workflow_dir: str = "workflow",
    include_workspaces: bool = False,
) -> None:
    config: Dict[str, Any] = {
        "air": {"dir": "air"},
        "modules": {"pull": False},
        "version": 1,
    }
    if (world_root / workflow_dir).exists():
        config["build"] = {"workflow_dir": workflow_dir}
    if include_workspaces:
        config["workspaces"] = [
            {
                "dir": workflow_dir,
                "ignore": ["target/", ".git/", ".aos/"],
                "ref": "workflow",
            }
        ]
    (world_root / "aos.sync.json").write_text(json.dumps(config, indent=2))


def _resolve_repo_root(repo_root: Optional[PathLike]) -> Path:
    if repo_root is not None:
        return _coerce_path(repo_root)

    env_root = os.environ.get("AOS_REPO_ROOT")
    if env_root:
        return _coerce_path(env_root)

    for base in (Path.cwd(), Path(__file__).resolve()):
        for candidate in (base, *base.parents):
            if (candidate / "crates" / "aos-smoke" / "fixtures").is_dir():
                return candidate
    raise FileNotFoundError(
        "could not locate repo root; pass repo_root=... or set AOS_REPO_ROOT"
    )


def repo_root(repo_root: Optional[PathLike] = None) -> Path:
    return _resolve_repo_root(repo_root)


def smoke_fixture_root(
    fixture_name: str,
    *,
    repo_root: Optional[PathLike] = None,
) -> Path:
    root = _resolve_repo_root(repo_root)
    return root / "crates" / "aos-smoke" / "fixtures" / fixture_name


@dataclass
class StagedWorld:
    root: Path

    def cleanup(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)

    def open_harness(
        self,
        *,
        effect_mode: EffectModeName = "scripted",
        reset: bool = True,
        force_build: bool = False,
        sync_secrets: bool = False,
    ) -> "WorldHarness":
        from ._core import WorldHarness

        return WorldHarness.from_world_dir(
            str(self.root),
            reset=reset,
            force_build=force_build,
            sync_secrets=sync_secrets,
            effect_mode=effect_mode,
        )

    def __enter__(self) -> "StagedWorld":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.cleanup()


def stage_authored_world(
    source_root: PathLike,
    *,
    sdk_root: Optional[PathLike] = None,
    include_workspaces: bool = False,
    temp_prefix: str = "aos-harness-py-",
) -> StagedWorld:
    source_root = _coerce_path(source_root)
    staged_root = Path(tempfile.mkdtemp(prefix=temp_prefix))
    _copy_authored_tree(source_root, staged_root)
    _patch_workflow_sdk_path(
        staged_root / "workflow",
        _coerce_path(sdk_root) if sdk_root is not None else None,
    )
    if not (staged_root / "aos.sync.json").exists():
        _write_minimal_sync_config(
            staged_root,
            include_workspaces=include_workspaces,
        )
    return StagedWorld(staged_root)


def stage_smoke_fixture(
    fixture_name: str,
    *,
    repo_root: Optional[PathLike] = None,
    include_workspaces: bool = False,
) -> StagedWorld:
    root = _resolve_repo_root(repo_root)
    fixture_root = smoke_fixture_root(fixture_name, repo_root=root)
    sdk_root = root / "crates" / "aos-wasm-sdk"
    return stage_authored_world(
        fixture_root,
        sdk_root=sdk_root,
        include_workspaces=include_workspaces,
        temp_prefix=f"aos-harness-{fixture_name}-",
    )
