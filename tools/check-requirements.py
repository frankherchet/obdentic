#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["pyyaml==6.0.3", "jsonschema==4.26.0"]
# [tool.uv]
# exclude-newer = "2026-11-01T00:00:00Z"
# ///
"""Checks the requirements file against its schema and for ID consistency (#372, REQ-056).

Usage, from the repository root: uv run --script tools/check-requirements.py [requirements.yaml] [schema.json]
Prints one line per problem and exits 1 when there is any.
"""
import json
import sys

import jsonschema
import yaml


def schema_problems(document, schema):
    validator = jsonschema.Draft202012Validator(schema)
    for error in sorted(validator.iter_errors(document), key=lambda e: list(e.absolute_path)):
        yield f"{error.json_path}: {error.message} ({error.validator})"


def id_problems(requirements):
    known = {r.get("id") for r in requirements if isinstance(r, dict)}
    seen_requirements, seen_criteria = set(), set()
    for requirement in requirements:
        if not isinstance(requirement, dict):
            continue
        rid = requirement.get("id")
        if rid in seen_requirements:
            yield f"{rid}: requirement ID occurs more than once"
        seen_requirements.add(rid)
        for criterion in requirement.get("acceptance") or []:
            cid = criterion.get("id") if isinstance(criterion, dict) else None
            if cid is None:
                continue
            if cid in seen_criteria:
                yield f"{cid}: criterion ID occurs more than once"
            seen_criteria.add(cid)
            if not str(cid).startswith(f"{rid}."):
                yield f"{cid}: criterion ID does not start with its requirement ID {rid}."
        for dependency in requirement.get("depends_on") or []:
            if dependency not in known:
                yield f"{rid}: depends_on names {dependency}, which does not exist"


def main(path="docs/requirements.yaml", schema_path="docs/requirements.schema.json"):
    with open(schema_path, encoding="utf-8") as f:
        schema = json.load(f)
    try:
        with open(path, encoding="utf-8") as f:
            document = yaml.safe_load(f)
    except yaml.YAMLError as error:
        mark = getattr(error, "problem_mark", None)
        where = f"line {mark.line + 1}, column {mark.column + 1}" if mark else "unknown line"
        print(f"{path}: {where}: invalid YAML: {getattr(error, 'problem', error)}")
        return 1
    problems = list(schema_problems(document, schema))
    if isinstance(document, dict) and isinstance(document.get("requirements"), list):
        problems += id_problems(document["requirements"])
    for problem in problems:
        print(f"{path}: {problem}")
    if problems:
        return 1
    print(f"{path}: valid")
    return 0


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:3]))
