from __future__ import annotations

from contextlib import contextmanager
from pathlib import Path
from typing import TYPE_CHECKING, Iterator, Optional

from .fixtures import smoke_fixture_root, stage_authored_world, stage_smoke_fixture
from .types import BuildProfileName, EffectModeName, PathLike

if TYPE_CHECKING:
    from ._core import WorkflowHarness, WorldHarness


@contextmanager
def world_from_authored_dir(
    source_root: PathLike,
    *,
    sdk_root: Optional[PathLike] = None,
    effect_mode: EffectModeName = "scripted",
    reset: bool = True,
    force_build: bool = False,
    sync_secrets: bool = False,
    include_workspaces: bool = False,
) -> Iterator["WorldHarness"]:
    with stage_authored_world(
        source_root,
        sdk_root=sdk_root,
        include_workspaces=include_workspaces,
    ) as world:
        yield world.open_harness(
            effect_mode=effect_mode,
            reset=reset,
            force_build=force_build,
            sync_secrets=sync_secrets,
        )


@contextmanager
def world_from_smoke_fixture(
    fixture_name: str,
    *,
    repo_root: Optional[PathLike] = None,
    effect_mode: EffectModeName = "scripted",
    reset: bool = True,
    force_build: bool = False,
    sync_secrets: bool = False,
    include_workspaces: bool = False,
) -> Iterator["WorldHarness"]:
    with stage_smoke_fixture(
        fixture_name,
        repo_root=repo_root,
        include_workspaces=include_workspaces,
    ) as world:
        yield world.open_harness(
            effect_mode=effect_mode,
            reset=reset,
            force_build=force_build,
            sync_secrets=sync_secrets,
        )


def workflow_from_authored_dir(
    source_root: PathLike,
    workflow: str,
    *,
    air_dir: str = "air",
    workflow_dir: str = "workflow",
    import_roots: Optional[list[str]] = None,
    force_build: bool = False,
    sync_secrets: bool = False,
    secret_bindings: Optional[dict[str, bytes | str]] = None,
    build_profile: BuildProfileName = "debug",
    effect_mode: EffectModeName = "scripted",
) -> "WorkflowHarness":
    from ._core import WorkflowHarness

    source_root = Path(source_root).expanduser().resolve()
    return WorkflowHarness.from_workflow_dir(
        workflow,
        str(source_root / workflow_dir),
        air_dir=str(source_root / air_dir),
        import_roots=import_roots,
        force_build=force_build,
        sync_secrets=sync_secrets,
        secret_bindings=secret_bindings,
        build_profile=build_profile,
        effect_mode=effect_mode,
    )


def workflow_from_smoke_fixture(
    fixture_name: str,
    workflow: str,
    *,
    repo_root: Optional[PathLike] = None,
    import_roots: Optional[list[str]] = None,
    force_build: bool = False,
    sync_secrets: bool = False,
    secret_bindings: Optional[dict[str, bytes | str]] = None,
    build_profile: BuildProfileName = "debug",
    effect_mode: EffectModeName = "scripted",
) -> "WorkflowHarness":
    fixture_root = smoke_fixture_root(fixture_name, repo_root=repo_root)
    return workflow_from_authored_dir(
        fixture_root,
        workflow,
        import_roots=import_roots,
        force_build=force_build,
        sync_secrets=sync_secrets,
        secret_bindings=secret_bindings,
        build_profile=build_profile,
        effect_mode=effect_mode,
    )
