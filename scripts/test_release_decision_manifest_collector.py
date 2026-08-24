#!/usr/bin/env python3
from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


CLIENT_ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = CLIENT_ROOT / "scripts" / "release_decision_manifest.py"
SPEC = importlib.util.spec_from_file_location("release_decision_manifest", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
manifest = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manifest)

COMMIT_SHA = "0123456789abcdef0123456789abcdef01234567"
RELEASE_TAG = "v0.0.1-rc.10"
OBSERVED_AT = "2026-08-24T13:00:00Z"
NOW = "2026-08-24T13:01:00Z"


class ReleaseDecisionManifestCollectorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = json.loads(
            manifest.DEFAULT_CHECKLIST_REGISTRY_PATH.read_text(encoding="utf-8")
        )
        self.receipt_index, self.human_results = self._payloads()

    def _receipt(self, item_id: str, *, human: bool) -> dict[str, str]:
        prefix = "issues/1#" if human else "actions/runs/1#"
        uri = f"https://github.com/pseudotop/maekon-client/{prefix}{item_id.lower()}"
        digest = hashlib.sha256(
            f"{item_id}\n{COMMIT_SHA}\n{RELEASE_TAG}\n{uri}".encode()
        ).hexdigest()
        return {
            "uri": uri,
            "sha256": digest,
            "observed_at": OBSERVED_AT,
            "commit_sha": COMMIT_SHA,
            "release_tag": RELEASE_TAG,
        }

    def _payloads(self) -> tuple[dict[str, object], dict[str, object]]:
        receipt_items: list[dict[str, object]] = []
        human_items: list[dict[str, object]] = []
        for registered in self.registry["items"]:
            phase = registered.get("phase", self.registry["default_phase"])
            is_human = registered["disposition"] == "human"
            item: dict[str, object] = {
                "id": registered["id"],
                "disposition": registered["disposition"],
                "subject_ref": registered["subject"]["ref"],
                "state": "pass" if phase == "pre_publish" or is_human else "pending",
                "receipt": self._receipt(registered["id"], human=is_human),
            }
            if is_human:
                item["reviewer"] = "pseudotop"
                human_items.append(item)
            else:
                receipt_items.append(item)

        return (
            {
                "schema_version": manifest.CHECKLIST_RECEIPT_INDEX_SCHEMA_VERSION,
                "commit_sha": COMMIT_SHA,
                "release_tag": RELEASE_TAG,
                "items": receipt_items,
            },
            {
                "schema_version": manifest.CHECKLIST_HUMAN_RESULTS_SCHEMA_VERSION,
                "commit_sha": COMMIT_SHA,
                "release_tag": RELEASE_TAG,
                "items": human_items,
            },
        )

    def _collect(
        self,
        receipt_index: dict[str, object] | None = None,
        human_results: dict[str, object] | None = None,
    ) -> dict[str, object]:
        return manifest.collect_checklist_results(
            receipt_index=receipt_index or self.receipt_index,
            human_results=human_results or self.human_results,
            commit_sha=COMMIT_SHA,
            release_tag=RELEASE_TAG,
            now=NOW,
        )

    def test_collects_all_results_once_in_canonical_order(self) -> None:
        result = self._collect()
        expected_ids = [item["id"] for item in self.registry["items"]]
        actual_ids = [item["id"] for item in result["items"]]

        self.assertEqual(actual_ids, expected_ids)
        self.assertEqual(len(actual_ids), 69)
        self.assertEqual(len(self.receipt_index["items"]), 65)
        self.assertEqual(len(self.human_results["items"]), 4)
        self.assertEqual(
            result["collector_schema_version"],
            manifest.CHECKLIST_COLLECTOR_SCHEMA_VERSION,
        )
        record = manifest._build_checklist_record(
            checklist_path=manifest.DEFAULT_CHECKLIST_PATH,
            registry_path=manifest.DEFAULT_CHECKLIST_REGISTRY_PATH,
            results=result,
            commit_sha=COMMIT_SHA,
            release_tag=RELEASE_TAG,
        )
        self.assertEqual(record["item_count"], 69)
        self.assertEqual(record["collection"]["commit_sha"], COMMIT_SHA)

    def test_rejects_missing_machine_or_evidence_receipt(self) -> None:
        mutated = copy.deepcopy(self.receipt_index)
        mutated["items"].pop()

        with self.assertRaisesRegex(SystemExit, "coverage mismatch: missing ids"):
            self._collect(receipt_index=mutated)

    def test_rejects_receipt_bound_to_a_different_commit(self) -> None:
        mutated = copy.deepcopy(self.receipt_index)
        mutated["items"][0]["receipt"]["commit_sha"] = "f" * 40

        with self.assertRaisesRegex(SystemExit, "commit_sha does not match"):
            self._collect(receipt_index=mutated)

    def test_rejects_placeholder_receipt_uri(self) -> None:
        mutated = copy.deepcopy(self.receipt_index)
        mutated["items"][0]["receipt"]["uri"] = "artifact://checklist/RC-AUTO-001"

        with self.assertRaisesRegex(SystemExit, "uri is a placeholder"):
            self._collect(receipt_index=mutated)

    def test_rejects_placeholder_human_reviewer(self) -> None:
        mutated = copy.deepcopy(self.human_results)
        mutated["items"][0]["reviewer"] = "release-maintainer"

        with self.assertRaisesRegex(SystemExit, "reviewer is a placeholder"):
            self._collect(human_results=mutated)

    def test_collected_results_cannot_be_rebound_during_manifest_build(self) -> None:
        collected = self._collect()

        with self.assertRaisesRegex(SystemExit, "commit_sha must match the manifest"):
            manifest._build_checklist_record(
                checklist_path=manifest.DEFAULT_CHECKLIST_PATH,
                registry_path=manifest.DEFAULT_CHECKLIST_REGISTRY_PATH,
                results=collected,
                commit_sha="f" * 40,
                release_tag=RELEASE_TAG,
            )

    def test_collected_results_require_manifest_binding_arguments(self) -> None:
        collected = self._collect()

        with self.assertRaisesRegex(
            SystemExit,
            "validation requires commit_sha and release_tag",
        ):
            manifest._build_checklist_record(
                checklist_path=manifest.DEFAULT_CHECKLIST_PATH,
                registry_path=manifest.DEFAULT_CHECKLIST_REGISTRY_PATH,
                results=collected,
            )

    def test_cli_writes_canonical_results(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            temp = Path(temp_dir)
            receipt_path = temp / "receipt-index.json"
            human_path = temp / "human-results.json"
            output_path = temp / "checklist-results.json"
            receipt_path.write_text(json.dumps(self.receipt_index), encoding="utf-8")
            human_path.write_text(json.dumps(self.human_results), encoding="utf-8")

            result = manifest.main(
                [
                    "collect-checklist-results",
                    "--receipt-index",
                    str(receipt_path),
                    "--human-results",
                    str(human_path),
                    "--commit-sha",
                    COMMIT_SHA,
                    "--release-tag",
                    RELEASE_TAG,
                    "--now",
                    NOW,
                    "--output",
                    str(output_path),
                ]
            )

            self.assertEqual(result, 0)
            output = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertEqual(len(output["items"]), 69)
            self.assertEqual(output["commit_sha"], COMMIT_SHA)


if __name__ == "__main__":
    unittest.main()
