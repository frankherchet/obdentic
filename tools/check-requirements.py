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


class UniqueKeyLoader(yaml.SafeLoader):
    """YAML requires unique mapping keys; safe_load would silently keep only the last value (a bad merge resolution)."""

    def construct_mapping(self, node, deep=False):
        seen = set()
        for key_node, _ in node.value:
            key = self.construct_object(key_node, deep=deep)
            if key in seen:
                raise yaml.constructor.ConstructorError(None, None, f"duplicate key '{key}'", key_node.start_mark)
            seen.add(key)
        return super().construct_mapping(node, deep=deep)


def schema_problems(document, schema):
    validator = jsonschema.Draft202012Validator(schema)
    for error in sorted(validator.iter_errors(document), key=lambda e: [str(p) for p in e.absolute_path]):
        yield f"{error.json_path}: {error.message} ({error.validator})"


def items(value):
    """The elements of a list; anything else (the schema reports it) has none."""
    return value if isinstance(value, list) else []


def text_id(value):
    """An ID only when it is a string; other shapes are the schema's to report."""
    value = value.get("id") if isinstance(value, dict) else None
    return value if isinstance(value, str) else None


def id_problems(requirements):
    known = {text_id(r) for r in requirements} - {None}
    seen_requirements, seen_criteria = set(), set()
    for requirement in requirements:
        rid = text_id(requirement)
        if rid is None:
            continue
        if rid in seen_requirements:
            yield f"{rid}: requirement ID occurs more than once"
        seen_requirements.add(rid)
        for criterion in items(requirement.get("acceptance")):
            cid = text_id(criterion)
            if cid is None:
                continue
            if cid in seen_criteria:
                yield f"{cid}: criterion ID occurs more than once"
            seen_criteria.add(cid)
            if not cid.startswith(f"{rid}."):
                yield f"{cid}: criterion ID does not start with its requirement ID {rid}."
        for dependency in items(requirement.get("depends_on")):
            if isinstance(dependency, str) and dependency not in known:
                yield f"{rid}: depends_on names {dependency}, which does not exist"


def main(path="docs/requirements.yaml", schema_path="docs/requirements.schema.json"):
    with open(schema_path, encoding="utf-8") as f:
        schema = json.load(f)
    try:
        with open(path, encoding="utf-8") as f:
            document = yaml.load(f, Loader=UniqueKeyLoader)
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
