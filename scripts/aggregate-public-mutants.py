#!/usr/bin/env python3
"""Verify complete, exact-public-commit mutation evidence before canonical scoring."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

REPOSITORY = "pseudotop/maekon-client"
CRATE = "maekon-core"
VERSION = "27.1.0"
TARGET = "x86_64-unknown-linux-gnu"
KINDS = {
    "CaughtMutant": "caught",
    "MissedMutant": "missed",
    "Timeout": "timeout",
    "Unviable": "unviable",
}
MUTANT_FIELDS = {"name", "package", "file", "function", "span", "replacement", "genre"}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def read_json(path: Path):
    require(path.stat().st_size <= 64 * 1024 * 1024, f"oversized evidence: {path}")
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def output(*argv: str) -> str:
    return subprocess.check_output(argv, text=True).strip()


def identity(source_sha: str, shards: int) -> dict:
    require(
        re.fullmatch(r"[0-9a-f]{40}", source_sha) is not None, "expected an exact SHA"
    )
    require(shards in (2, 4, 8, 16), "shards must be 2, 4, 8 or 16")
    for key, expected in {
        "GITHUB_REPOSITORY": REPOSITORY,
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_ACTOR": "pseudotop",
        "GITHUB_TRIGGERING_ACTOR": "pseudotop",
        "GITHUB_SHA": source_sha,
    }.items():
        require(os.environ.get(key) == expected, f"unexpected {key}")
    require(output("git", "rev-parse", "HEAD") == source_sha, "checkout SHA mismatch")
    run_id, attempt = os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"]
    require(run_id.isdigit() and int(run_id) > 0, "invalid run ID")
    require(attempt.isdigit() and int(attempt) > 0, "invalid run attempt")
    return {
        "repository": REPOSITORY,
        "source_sha": source_sha,
        "run_id": run_id,
        "run_attempt": attempt,
        "run_url": f"https://github.com/{REPOSITORY}/actions/runs/{run_id}/attempts/{attempt}",
        "crate": CRATE,
        "scope": "",
        "shards": shards,
    }


def toolchain() -> dict:
    require(os.environ.get("CARGO_BUILD_TARGET") == TARGET, "unexpected build target")
    require(
        output("cargo", "mutants", "--version") == f"cargo-mutants {VERSION}",
        "unexpected cargo-mutants version",
    )
    require(os.environ.get("CARGO_INCREMENTAL") == "1", "incremental profile mismatch")
    require(
        os.environ.get("CARGO_PROFILE_TEST_DEBUG") == "0", "test debug profile mismatch"
    )
    require(not os.environ.get("RUSTC_WRAPPER"), "unexpected Rust wrapper")
    return {
        "cargo_mutants_version": VERSION,
        "rustc": output("rustc", "--version", "--verbose"),
        "cargo": output("cargo", "--version"),
        "target": TARGET,
        "jobs_per_shard": 2,
        "shard_max_parallel": 2,
        "cargo_incremental": 1,
        "test_debuginfo": 0,
        "rustc_wrapper": "disabled",
    }


def mutant_key(mutant: dict) -> str:
    # --list --json adds a diff; scenario.Mutant serialization omits it.
    require(
        isinstance(mutant, dict) and MUTANT_FIELDS <= mutant.keys(),
        "invalid mutant identity",
    )
    require(mutant["package"] == CRATE, "foreign crate mutant")
    encoded = json.dumps({key: mutant[key] for key in MUTANT_FIELDS}, sort_keys=True)
    return hashlib.sha256(encoded.encode()).hexdigest()


def mutant_set(items: list) -> set[str]:
    require(isinstance(items, list) and bool(items), "empty or invalid enumeration")
    keys = [mutant_key(item) for item in items]
    require(len(keys) == len(set(keys)), "duplicate mutant identity")
    return set(keys)


def timestamp(value: str) -> datetime:
    require(isinstance(value, str), "missing completion timestamp")
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    require(parsed.tzinfo is not None, "timestamp has no timezone")
    return parsed


def phase_statuses(outcome: dict) -> list:
    phases = outcome.get("phase_results", [])
    require(isinstance(phases, list) and bool(phases), "missing phase results")
    names = [phase["phase"] for phase in phases]
    require(names in (["Build"], ["Build", "Test"]), "unexpected build/test phases")
    return [phase["process_status"] for phase in phases]


def is_failure(status) -> bool:
    return (
        isinstance(status, dict)
        and set(status) == {"Failure"}
        and type(status["Failure"]) is int
        and status["Failure"] != 0
    )


def verify_outcomes(document: dict, expected: set[str]) -> dict[str, list[str]]:
    require(isinstance(document, dict), "invalid outcome document")
    require(
        document.get("cargo_mutants_version") == VERSION,
        "outcome tool version mismatch",
    )
    require(
        timestamp(document.get("end_time")) >= timestamp(document.get("start_time")),
        "invalid run times",
    )
    rows = document["outcomes"]
    require(
        isinstance(rows, list) and all(isinstance(row, dict) for row in rows),
        "invalid outcomes",
    )
    baselines = [row for row in rows if row.get("scenario") == "Baseline"]
    require(len(baselines) == 1, "expected exactly one baseline")
    require(
        baselines[0].get("summary") == "Success"
        and phase_statuses(baselines[0]) == ["Success", "Success"],
        "baseline build and tests must pass",
    )
    classified = {kind: [] for kind in KINDS.values()}
    seen = set()
    for row in rows:
        if row.get("scenario") == "Baseline":
            continue
        scenario = row.get("scenario")
        require(
            isinstance(scenario, dict) and set(scenario) == {"Mutant"},
            "unknown scenario",
        )
        key = mutant_key(scenario["Mutant"])
        require(key not in seen, "duplicate mutant outcome")
        seen.add(key)
        summary, statuses = row.get("summary"), phase_statuses(row)
        require(summary in KINDS, "unclassified mutant outcome")
        if summary == "CaughtMutant":
            valid = (
                len(statuses) == 2
                and statuses[0] == "Success"
                and is_failure(statuses[1])
            )
        elif summary == "MissedMutant":
            valid = statuses == ["Success", "Success"]
        elif summary == "Unviable":
            valid = len(statuses) == 1 and is_failure(statuses[0])
        else:
            valid = statuses == ["Timeout"] or statuses == ["Success", "Timeout"]
        require(valid, "summary contradicts build/test outcome")
        classified[KINDS[summary]].append(key)
    require(seen == expected, "incomplete or foreign mutant coverage")
    for kind, keys in classified.items():
        require(
            type(document.get(kind)) is int and document[kind] == len(keys),
            f"raw {kind} count mismatch",
        )
    require(
        type(document.get("total_mutants")) is int
        and document["total_mutants"] == len(seen),
        "raw total mismatch",
    )
    require(document.get("success") == 0, "unclassified successful mutants")
    return classified


def aggregate(
    shards_dir: Path,
    enumeration: Path,
    destination: Path,
    provenance: dict,
    versions: dict,
    shard_result: str,
) -> int:
    receipt = {**provenance, "toolchain": versions, "status": "failed"}
    destination.mkdir(parents=True, exist_ok=True)
    try:
        require(shard_result == "success", "one or more shard jobs failed")
        shard_names = {f"mutants-shard-{i}" for i in range(provenance["shards"])}
        require(
            {path.name for path in shards_dir.iterdir()} == shard_names,
            "missing or foreign shard artifact",
        )
        whole = mutant_set(read_json(enumeration))
        merged = {kind: [] for kind in KINDS.values()}
        shard_exits = {}
        for index in range(provenance["shards"]):
            shard = shards_dir / f"mutants-shard-{index}"
            metadata = read_json(shard / "metadata.json")
            require(isinstance(metadata, dict), "invalid shard metadata")
            for key, value in {
                **provenance,
                "shard": index,
                "toolchain": versions,
            }.items():
                require(
                    type(metadata.get(key)) is type(value) and metadata[key] == value,
                    f"shard {index}: {key} mismatch",
                )
            require(
                type(metadata.get("cargo_mutants_exit")) is int, "missing raw exit code"
            )
            shard_exits[str(index)] = metadata["cargo_mutants_exit"]
            expected = mutant_set(read_json(shard / "enumeration.json"))
            outcomes = read_json(shard / "mutants.out" / "outcomes.json")
            classified = verify_outcomes(outcomes, expected)
            require(
                timestamp(metadata["started_at"])
                <= timestamp(outcomes.get("start_time"))
                <= timestamp(outcomes.get("end_time"))
                <= timestamp(metadata["recorded_at"]),
                "outcomes outside this shard's execution window",
            )
            for kind, keys in classified.items():
                merged[kind].extend(keys)
        counts = Counter(key for keys in merged.values() for key in keys)
        require(
            all(count == 1 for count in counts.values()),
            "duplicate mutant across shards",
        )
        require(set(counts) == whole, "whole-core enumeration coverage mismatch")
        receipt.update(
            enumerated=len(whole),
            merged_total=len(counts),
            shard_set=list(range(provenance["shards"])),
            shard_exit_codes=shard_exits,
        )
        # Keep score arithmetic and timeout treatment in the release's canonical gate.
        for kind, keys in merged.items():
            (destination / f"{kind}.txt").write_text(
                "".join(f"{key}\n" for key in keys), encoding="utf-8"
            )
        score_file = destination / "canonical-score.json"
        result = subprocess.run(
            [
                "bash",
                str(Path(__file__).with_name("run-mutants.sh")),
                "--score-only",
                CRATE,
            ],
            env={
                **os.environ,
                "MUTANTS_OUT_DIR": str(destination.resolve()),
                "MUTANTS_MIN_SCORE": "70",
                "MUTANTS_RECEIPT": str(score_file.resolve()),
            },
            check=False,
        )
        require(
            result.returncode in (0, 1) and score_file.is_file(),
            "canonical scorer failed",
        )
        receipt.update(read_json(score_file))
        receipt["cargo_mutants_exit"] = (
            shard_exits  # --score-only did not run cargo-mutants.
        )
        receipt["status"] = "passed" if result.returncode == 0 else "failed"
        return result.returncode
    except (ValueError, KeyError, TypeError, OSError) as exc:
        receipt["error"] = str(exc)
        print(f"::error::{exc}")
        return 2
    finally:
        write_json(destination / "mutants-score.final.json", receipt)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for command in ("plan", "record-shard", "aggregate"):
        sub = commands.add_parser(command)
        sub.add_argument("--source-sha", required=True)
        sub.add_argument("--shards", type=int, required=True)
        if command != "plan":
            sub.add_argument("--output", type=Path, required=True)
        if command == "record-shard":
            sub.add_argument("--shard", type=int, required=True)
            sub.add_argument("--exit-code", type=int, required=True)
            sub.add_argument("--started-at", required=True)
        if command == "aggregate":
            sub.add_argument("--shards-dir", type=Path, required=True)
            sub.add_argument("--enumeration", type=Path, required=True)
            sub.add_argument("--shard-result", required=True)
    args = parser.parse_args()
    try:
        provenance = identity(args.source_sha, args.shards)
        if args.command == "plan":
            with Path(os.environ["GITHUB_OUTPUT"]).open(
                "a", encoding="utf-8"
            ) as stream:
                stream.write(f"resolved_sha={args.source_sha}\n")
                stream.write(
                    "matrix=" + json.dumps({"shard": list(range(args.shards))}) + "\n"
                )
            return 0
        versions = toolchain()
        if args.command == "record-shard":
            require(0 <= args.shard < args.shards, "shard outside plan")
            timestamp(args.started_at)
            write_json(
                args.output / "metadata.json",
                {
                    **provenance,
                    "shard": args.shard,
                    "toolchain": versions,
                    "cargo_mutants_exit": args.exit_code,
                    "started_at": args.started_at,
                    "recorded_at": datetime.now(timezone.utc).isoformat(),
                },
            )
            return 0
        return aggregate(
            args.shards_dir,
            args.enumeration,
            args.output,
            provenance,
            versions,
            args.shard_result,
        )
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as exc:
        print(f"::error::{exc}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
